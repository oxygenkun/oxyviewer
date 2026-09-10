use oxy_domain::{
    AssetKind, AssetQuery, AssetSort, AssetSummary, DirectorySearchMatch, DirectorySummary,
    ImageProjection, MetadataProjection, Page, PickLabel, RenderLevel, ResourceLoadStatus,
    SortDirection,
};
use parking_lot::{Mutex, MutexGuard};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;

mod browsing;
mod tags;
pub use browsing::DirectoryRead;

const DEFAULT_PAGE_SIZE: usize = 250;
const MAX_PAGE_SIZE: usize = 1_000;
const INDEX_WRITE_BATCH_SIZE: usize = 256;
const INDEX_WRITE_TIME_SLICE: Duration = Duration::from_millis(8);

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("directory snapshot changed; reload the first page")]
    StaleDirectorySnapshot,
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Filesystem(#[from] oxy_fs::FsError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("folder order must contain every library root exactly once")]
    InvalidRootOrder,
    #[error("tag name must not be empty or contain '|'")]
    InvalidTagName,
    #[error("tag parent does not exist")]
    MissingTagParent,
    #[error("a tag cannot be moved below itself")]
    TagHierarchyCycle,
    #[error("a tag with this name already exists at this level")]
    DuplicateTagName,
}

pub struct Library {
    directory_snapshots: browsing::DirectorySnapshots,
    pub foreground: oxy_runtime::ForegroundGate,
    connection: Arc<Mutex<Connection>>,
    // Disk libraries use WAL readers that never acquire the writer mutex.
    // Plain in-memory databases cannot share WAL; tests retain one connection.
    reader: Option<Mutex<Connection>>,
    projection_reader: Option<Mutex<Connection>>,
    indexing_roots: Mutex<HashSet<PathBuf>>,
    index_gate: Mutex<()>,
}

/// Give FTS records an indexed, stable identity. FTS5 cannot efficiently look up
/// equality predicates on its UNINDEXED path columns. Preserve existing rowids
/// during migration so an interrupted index can resume without rebuilding search.
fn ensure_search_keys(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    let transaction = connection.transaction()?;
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table'
         AND name = 'indexed_asset_search_keys')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        transaction.execute_batch(
            "CREATE TABLE indexed_asset_search_keys (
               search_rowid INTEGER PRIMARY KEY,
               root_path TEXT NOT NULL,
               path TEXT NOT NULL,
               UNIQUE(root_path, path)
             );
             INSERT INTO indexed_asset_search_keys(search_rowid, root_path, path)
               SELECT rowid, root_path, path FROM indexed_asset_search;",
        )?;
    }
    transaction.commit()
}

fn normalize_root_order(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    let paths = {
        let mut statement = connection.prepare(
            "SELECT path FROM library_roots
             ORDER BY sort_order IS NULL, sort_order, added_at, path",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let transaction = connection.transaction()?;
    for (sort_order, path) in paths.iter().enumerate() {
        transaction.execute(
            "UPDATE library_roots SET sort_order = ?1 WHERE path = ?2",
            params![sort_order as i64, path],
        )?;
    }
    transaction.commit()
}

#[derive(Debug, Eq, PartialEq)]
struct IndexQueueEntry {
    depth: usize,
    sequence: usize,
    path: PathBuf,
}

impl Ord for IndexQueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap pops the greatest item. A smaller depth represents a
        // higher directory level and therefore a greater indexing priority.
        other
            .depth
            .cmp(&self.depth)
            .then_with(|| other.sequence.cmp(&self.sequence))
            .then_with(|| self.path.cmp(&other.path))
    }
}

impl PartialOrd for IndexQueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct DirectoryPriorityQueue {
    root: PathBuf,
    pending: BinaryHeap<IndexQueueEntry>,
    visited: HashSet<PathBuf>,
    next_sequence: usize,
}

impl DirectoryPriorityQueue {
    fn new(root: PathBuf) -> Self {
        let pending = BinaryHeap::from([IndexQueueEntry {
            depth: 0,
            sequence: 0,
            path: root.clone(),
        }]);
        Self {
            pending,
            visited: HashSet::from([root.clone()]),
            root,
            next_sequence: 1,
        }
    }

    fn pop_next(&mut self) -> Option<(PathBuf, usize)> {
        self.pending.pop().map(|entry| (entry.path, entry.depth))
    }

    fn enqueue_children(
        &mut self,
        parent_depth: usize,
        directories: Vec<DirectorySummary>,
    ) -> Vec<DirectorySummary> {
        let mut safe_directories = directories
            .into_iter()
            .filter_map(|mut child| {
                let canonical = child.path.canonicalize().ok()?;
                if !canonical.starts_with(&self.root) || !self.visited.insert(canonical.clone()) {
                    return None;
                }
                child.path = canonical;
                Some(child)
            })
            .collect::<Vec<_>>();
        safe_directories.sort_unstable_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.path.cmp(&right.path))
        });
        for child in &safe_directories {
            self.pending.push(IndexQueueEntry {
                depth: parent_depth.saturating_add(1),
                sequence: self.next_sequence,
                path: child.path.clone(),
            });
            self.next_sequence = self.next_sequence.saturating_add(1);
        }
        safe_directories
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexStats {
    pub asset_count: usize,
    pub directory_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexProgress {
    pub root_path: PathBuf,
    pub current_directory: PathBuf,
    pub pending_directory_count: usize,
    pub asset_count: usize,
    pub directory_count: usize,
    pub stage: IndexStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexStage {
    Directories,
    Assets,
}

impl Library {
    fn read_connection(&self) -> MutexGuard<'_, Connection> {
        self.reader.as_ref().unwrap_or(&self.connection).lock()
    }

    fn read_projection_connection(&self) -> MutexGuard<'_, Connection> {
        self.projection_reader
            .as_ref()
            .unwrap_or(&self.connection)
            .lock()
    }

    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS library_roots (
              path TEXT PRIMARY KEY NOT NULL,
              added_at INTEGER NOT NULL DEFAULT (unixepoch()),
              sort_order INTEGER
            );
            CREATE TABLE IF NOT EXISTS indexed_roots (
              root_path TEXT PRIMARY KEY NOT NULL,
              indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
              asset_count INTEGER NOT NULL,
              directory_count INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS indexed_directory_roots (
              root_path TEXT PRIMARY KEY NOT NULL,
              indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
              directory_count INTEGER NOT NULL
            );
            INSERT OR IGNORE INTO indexed_directory_roots(root_path, indexed_at, directory_count)
              SELECT root_path, indexed_at, directory_count FROM indexed_roots;
            CREATE TABLE IF NOT EXISTS library_index_sequence (
              id INTEGER PRIMARY KEY CHECK(id = 1),
              next_scan_id INTEGER NOT NULL
            );
            INSERT OR IGNORE INTO library_index_sequence(id, next_scan_id) VALUES (1, 1);
            CREATE TABLE IF NOT EXISTS indexed_assets (
              root_path TEXT NOT NULL,
              path TEXT NOT NULL,
              parent_path TEXT NOT NULL,
              id TEXT NOT NULL,
              name TEXT NOT NULL,
              extension TEXT NOT NULL,
              kind TEXT NOT NULL,
              modified_at_ms INTEGER NOT NULL,
              size_bytes INTEGER NOT NULL,
              has_sidecar INTEGER NOT NULL,
              scan_id INTEGER NOT NULL,
              PRIMARY KEY(root_path, path)
            );
            CREATE INDEX IF NOT EXISTS indexed_assets_parent
              ON indexed_assets(root_path, parent_path);
            CREATE TABLE IF NOT EXISTS indexed_directories (
              root_path TEXT NOT NULL,
              path TEXT NOT NULL,
              parent_path TEXT NOT NULL,
              name TEXT NOT NULL,
              has_children INTEGER NOT NULL,
              scan_id INTEGER NOT NULL,
              PRIMARY KEY(root_path, path)
            );
            CREATE INDEX IF NOT EXISTS indexed_directories_parent
              ON indexed_directories(root_path, parent_path);
            CREATE VIRTUAL TABLE IF NOT EXISTS indexed_asset_search USING fts5(
              path UNINDEXED,
              root_path UNINDEXED,
              name,
              directory,
              tokenize = 'unicode61 remove_diacritics 2'
            );
            CREATE TABLE IF NOT EXISTS resource_projection_sequence (
              id INTEGER PRIMARY KEY CHECK(id = 1),
              next_revision INTEGER NOT NULL
            );
            INSERT OR IGNORE INTO resource_projection_sequence(id, next_revision) VALUES (1, 1);
            CREATE TABLE IF NOT EXISTS resource_projections (
              path TEXT NOT NULL,
              parent_path TEXT NOT NULL,
              projection_kind TEXT NOT NULL,
              source_revision TEXT NOT NULL,
              valid_at INTEGER NOT NULL,
              state_revision INTEGER NOT NULL,
              status TEXT NOT NULL,
              rating INTEGER,
              color_label TEXT,
              pick_label TEXT,
              result_json TEXT,
              error TEXT,
              PRIMARY KEY(path, projection_kind)
            );
            CREATE INDEX IF NOT EXISTS resource_projections_parent
              ON resource_projections(parent_path);
            CREATE TABLE IF NOT EXISTS custom_tags (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              parent_id INTEGER REFERENCES custom_tags(id) ON DELETE CASCADE,
              name TEXT NOT NULL,
              name_key TEXT NOT NULL,
              sort_order INTEGER NOT NULL,
              created_at INTEGER NOT NULL DEFAULT (unixepoch()),
              updated_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE UNIQUE INDEX IF NOT EXISTS custom_tags_sibling_name
              ON custom_tags(COALESCE(parent_id, 0), name_key);
            CREATE INDEX IF NOT EXISTS custom_tags_parent
              ON custom_tags(parent_id, sort_order, name);
            CREATE TABLE IF NOT EXISTS asset_tags (
              asset_path TEXT NOT NULL,
              tag_id INTEGER NOT NULL REFERENCES custom_tags(id) ON DELETE CASCADE,
              assigned_at INTEGER NOT NULL DEFAULT (unixepoch()),
              PRIMARY KEY(asset_path, tag_id)
            );
            CREATE INDEX IF NOT EXISTS asset_tags_tag ON asset_tags(tag_id, asset_path);
            CREATE TABLE IF NOT EXISTS asset_tag_xmp_state (
              asset_path TEXT PRIMARY KEY NOT NULL,
              subjects_json TEXT NOT NULL DEFAULT '[]',
              hierarchical_json TEXT NOT NULL DEFAULT '[]',
              synced_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE TABLE IF NOT EXISTS tag_xmp_sync_queue (
              asset_path TEXT PRIMARY KEY NOT NULL,
              requested_at INTEGER NOT NULL DEFAULT (unixepoch()),
              attempt_count INTEGER NOT NULL DEFAULT 0,
              last_error TEXT
            );
            ",
        )?;
        let has_sort_order = connection
            .prepare("PRAGMA table_info(library_roots)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|column| column == "sort_order");
        if !has_sort_order {
            connection.execute(
                "ALTER TABLE library_roots ADD COLUMN sort_order INTEGER",
                [],
            )?;
        }
        let projection_columns = connection
            .prepare("PRAGMA table_info(resource_projections)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        let has_state_revision = projection_columns
            .iter()
            .any(|column| column == "state_revision");
        let has_legacy_projection_revision = projection_columns
            .iter()
            .any(|column| column == "projection_revision");
        if !has_state_revision && has_legacy_projection_revision {
            connection.execute(
                "ALTER TABLE resource_projections RENAME COLUMN projection_revision TO state_revision",
                [],
            )?;
        }
        let has_pick_label = projection_columns
            .iter()
            .any(|column| column == "pick_label");
        if !has_pick_label {
            connection.execute(
                "ALTER TABLE resource_projections ADD COLUMN pick_label TEXT",
                [],
            )?;
        }
        browsing::ensure_schema(&connection)?;
        ensure_search_keys(&mut connection)?;
        normalize_root_order(&mut connection)?;
        let open_reader = || -> Result<Mutex<Connection>, rusqlite::Error> {
            let reader = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            Ok(Mutex::new(reader))
        };
        let connection = Arc::new(Mutex::new(connection));
        Ok(Self {
            reader: Some(open_reader()?),
            projection_reader: Some(open_reader()?),
            connection: connection.clone(),
            indexing_roots: Mutex::new(HashSet::new()),
            index_gate: Mutex::new(()),
            directory_snapshots: browsing::DirectorySnapshots::new(connection),
            foreground: oxy_runtime::ForegroundGate::default(),
        })
    }

    pub fn in_memory() -> Result<Self, LibraryError> {
        let mut connection = Connection::open_in_memory()?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            "
            CREATE TABLE library_roots (
              path TEXT PRIMARY KEY NOT NULL,
              added_at INTEGER NOT NULL DEFAULT (unixepoch()),
              sort_order INTEGER
            );
            CREATE TABLE indexed_roots (
              root_path TEXT PRIMARY KEY NOT NULL,
              indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
              asset_count INTEGER NOT NULL,
              directory_count INTEGER NOT NULL
            );
            CREATE TABLE indexed_directory_roots (
              root_path TEXT PRIMARY KEY NOT NULL,
              indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
              directory_count INTEGER NOT NULL
            );
            CREATE TABLE library_index_sequence (
              id INTEGER PRIMARY KEY CHECK(id = 1),
              next_scan_id INTEGER NOT NULL
            );
            INSERT INTO library_index_sequence(id, next_scan_id) VALUES (1, 1);
            CREATE TABLE indexed_assets (
              root_path TEXT NOT NULL,
              path TEXT NOT NULL,
              parent_path TEXT NOT NULL,
              id TEXT NOT NULL,
              name TEXT NOT NULL,
              extension TEXT NOT NULL,
              kind TEXT NOT NULL,
              modified_at_ms INTEGER NOT NULL,
              size_bytes INTEGER NOT NULL,
              has_sidecar INTEGER NOT NULL,
              scan_id INTEGER NOT NULL,
              PRIMARY KEY(root_path, path)
            );
            CREATE INDEX indexed_assets_parent ON indexed_assets(root_path, parent_path);
            CREATE TABLE indexed_directories (
              root_path TEXT NOT NULL,
              path TEXT NOT NULL,
              parent_path TEXT NOT NULL,
              name TEXT NOT NULL,
              has_children INTEGER NOT NULL,
              scan_id INTEGER NOT NULL,
              PRIMARY KEY(root_path, path)
            );
            CREATE INDEX indexed_directories_parent
              ON indexed_directories(root_path, parent_path);
            CREATE VIRTUAL TABLE indexed_asset_search USING fts5(
              path UNINDEXED,
              root_path UNINDEXED,
              name,
              directory,
              tokenize = 'unicode61 remove_diacritics 2'
            );
            CREATE TABLE resource_projection_sequence (
              id INTEGER PRIMARY KEY CHECK(id = 1),
              next_revision INTEGER NOT NULL
            );
            INSERT INTO resource_projection_sequence(id, next_revision) VALUES (1, 1);
            CREATE TABLE resource_projections (
              path TEXT NOT NULL,
              parent_path TEXT NOT NULL,
              projection_kind TEXT NOT NULL,
              source_revision TEXT NOT NULL,
              valid_at INTEGER NOT NULL,
              state_revision INTEGER NOT NULL,
              status TEXT NOT NULL,
              rating INTEGER,
              color_label TEXT,
              pick_label TEXT,
              result_json TEXT,
              error TEXT,
              PRIMARY KEY(path, projection_kind)
            );
            CREATE INDEX resource_projections_parent ON resource_projections(parent_path);
            CREATE TABLE custom_tags (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              parent_id INTEGER REFERENCES custom_tags(id) ON DELETE CASCADE,
              name TEXT NOT NULL,
              name_key TEXT NOT NULL,
              sort_order INTEGER NOT NULL,
              created_at INTEGER NOT NULL DEFAULT (unixepoch()),
              updated_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE UNIQUE INDEX custom_tags_sibling_name
              ON custom_tags(COALESCE(parent_id, 0), name_key);
            CREATE INDEX custom_tags_parent ON custom_tags(parent_id, sort_order, name);
            CREATE TABLE asset_tags (
              asset_path TEXT NOT NULL,
              tag_id INTEGER NOT NULL REFERENCES custom_tags(id) ON DELETE CASCADE,
              assigned_at INTEGER NOT NULL DEFAULT (unixepoch()),
              PRIMARY KEY(asset_path, tag_id)
            );
            CREATE INDEX asset_tags_tag ON asset_tags(tag_id, asset_path);
            CREATE TABLE asset_tag_xmp_state (
              asset_path TEXT PRIMARY KEY NOT NULL,
              subjects_json TEXT NOT NULL DEFAULT '[]',
              hierarchical_json TEXT NOT NULL DEFAULT '[]',
              synced_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE TABLE tag_xmp_sync_queue (
              asset_path TEXT PRIMARY KEY NOT NULL,
              requested_at INTEGER NOT NULL DEFAULT (unixepoch()),
              attempt_count INTEGER NOT NULL DEFAULT 0,
              last_error TEXT
            );
            ",
        )?;
        browsing::ensure_schema(&connection)?;
        ensure_search_keys(&mut connection)?;
        normalize_root_order(&mut connection)?;
        let connection = Arc::new(Mutex::new(connection));
        Ok(Self {
            connection: connection.clone(),
            reader: None,
            projection_reader: None,
            indexing_roots: Mutex::new(HashSet::new()),
            index_gate: Mutex::new(()),
            directory_snapshots: browsing::DirectorySnapshots::new(connection),
            foreground: oxy_runtime::ForegroundGate::default(),
        })
    }

    pub fn add_root(&self, path: &Path) -> Result<(), LibraryError> {
        let canonical = path.canonicalize()?;
        self.connection.lock().execute(
            "INSERT OR IGNORE INTO library_roots(path, sort_order)
             VALUES (?1, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM library_roots))",
            params![canonical.to_string_lossy()],
        )?;
        Ok(())
    }

    pub fn remove_root(&self, path: &Path) -> Result<(), LibraryError> {
        // Stored roots are canonical, but removal must also work after a folder
        // has been moved or disconnected. In that case the exact persisted path
        // is still safe to remove.
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.invalidate_snapshot_root(&path)?;
        let root = path.to_string_lossy();
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM library_roots WHERE path = ?1", params![root])?;
        transaction.execute(
            "DELETE FROM indexed_roots WHERE root_path = ?1",
            params![root],
        )?;
        transaction.execute(
            "DELETE FROM indexed_directory_roots WHERE root_path = ?1",
            params![root],
        )?;
        transaction.execute(
            "DELETE FROM indexed_assets WHERE root_path = ?1",
            params![root],
        )?;
        transaction.execute(
            "DELETE FROM indexed_directories WHERE root_path = ?1",
            params![root],
        )?;
        transaction.execute(
            "DELETE FROM indexed_asset_search WHERE rowid IN (
               SELECT search_rowid FROM indexed_asset_search_keys WHERE root_path = ?1
             )",
            params![root],
        )?;
        transaction.execute(
            "DELETE FROM indexed_asset_search_keys WHERE root_path = ?1",
            params![root],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn roots(&self) -> Result<Vec<PathBuf>, LibraryError> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare("SELECT path FROM library_roots ORDER BY sort_order, added_at, path")?;
        let paths = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(paths.into_iter().map(PathBuf::from).collect())
    }

    pub fn reorder_roots(&self, paths: &[PathBuf]) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let stored = connection
            .prepare("SELECT path FROM library_roots")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<HashSet<_>, _>>()?;
        let requested = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<HashSet<_>>();
        if stored.len() != paths.len() || requested != stored {
            return Err(LibraryError::InvalidRootOrder);
        }

        let transaction = connection.transaction()?;
        for (sort_order, path) in paths.iter().enumerate() {
            transaction.execute(
                "UPDATE library_roots SET sort_order = ?1 WHERE path = ?2",
                params![sort_order as i64, path.to_string_lossy()],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn contains_root(&self, path: &Path) -> Result<bool, LibraryError> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        self.read_connection()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM library_roots WHERE path = ?1)",
                params![path.to_string_lossy()],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    /// Allocates a revision in SQLite so observations from separate native
    /// processes share one comparable acceptance order.
    pub fn next_resource_revision(&self) -> Result<u64, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = next_resource_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    pub fn metadata_projection(
        &self,
        path: &Path,
        source_revision: &str,
    ) -> Result<Option<MetadataProjection>, LibraryError> {
        let connection = self.read_projection_connection();
        let projection = read_metadata_projection(&connection, path)?;
        Ok(projection.filter(|projection| projection.source_revision == source_revision))
    }

    /// Returns a stale-while-revalidate snapshot using only the source stat
    /// already present in an `AssetSummary`. Callers must still validate the
    /// complete revision, including sidecar or embedded metadata fingerprints.
    pub fn metadata_projection_for_asset(
        &self,
        asset: &AssetSummary,
    ) -> Result<Option<MetadataProjection>, LibraryError> {
        let connection = self.read_projection_connection();
        let prefix = format!("{}:{}:", asset.modified_at_ms, asset.size_bytes);
        Ok(
            read_metadata_projection(&connection, &asset.path)?.filter(|projection| {
                projection.status == ResourceLoadStatus::Ready
                    && projection.source_revision.starts_with(&prefix)
            }),
        )
    }

    /// Atomically accepts a metadata observation or returns the newer state
    /// that already won. The returned revision is the only revision sent to UI.
    pub fn accept_metadata_projection(
        &self,
        mut candidate: MetadataProjection,
    ) -> Result<MetadataProjection, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(current) = read_metadata_projection(&transaction, &candidate.path)?
            && current.valid_at > candidate.valid_at
        {
            transaction.commit()?;
            return Ok(current);
        }
        candidate.state_revision = next_resource_revision(&transaction)?;
        transaction.execute(
            "INSERT INTO resource_projections(
               path, parent_path, projection_kind, source_revision, valid_at,
               state_revision, status, rating, color_label, pick_label, result_json, error
             ) VALUES (?1, ?2, 'metadata', ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10)
             ON CONFLICT(path, projection_kind) DO UPDATE SET
               parent_path=excluded.parent_path,
               source_revision=excluded.source_revision,
               valid_at=excluded.valid_at,
               state_revision=excluded.state_revision,
               status=excluded.status,
               rating=excluded.rating,
               color_label=excluded.color_label,
               pick_label=excluded.pick_label,
               result_json=NULL,
               error=excluded.error",
            params![
                candidate.path.to_string_lossy(),
                parent_string(&candidate.path),
                candidate.source_revision,
                candidate.valid_at as i64,
                candidate.state_revision as i64,
                status_name(candidate.status),
                candidate.rating,
                candidate.color_label,
                candidate.pick_label.map(pick_label_name),
                candidate.error,
            ],
        )?;
        transaction.commit()?;
        Ok(candidate)
    }

    pub fn image_projection(
        &self,
        path: &Path,
        level: RenderLevel,
        source_revision: &str,
    ) -> Result<Option<ImageProjection>, LibraryError> {
        let connection = self.read_projection_connection();
        let projection = read_image_projection(&connection, path, level)?;
        Ok(projection.filter(|projection| projection.source_revision == source_revision))
    }

    /// Atomically accepts an image artifact observation using the same
    /// transaction sequence as metadata projections.
    pub fn accept_image_projection(
        &self,
        mut candidate: ImageProjection,
    ) -> Result<ImageProjection, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(current) =
            read_image_projection(&transaction, &candidate.path, candidate.level)?
            && current.valid_at > candidate.valid_at
        {
            transaction.commit()?;
            return Ok(current);
        }
        candidate.state_revision = next_resource_revision(&transaction)?;
        // Resource descriptors identify entries in the current process registry.
        // Persist only restart-safe artifact facts; a caller restoring a managed
        // file must register it again in its own process.
        let persisted_result = candidate.result.as_ref().and_then(|result| {
            let restart_safe = matches!(
                result.persistence,
                Some(oxy_domain::MediaPersistence::Persisted)
                    | Some(oxy_domain::MediaPersistence::NotApplicable)
                    | None
            ) && result.path.is_file();
            restart_safe.then(|| {
                let mut result = result.clone();
                result.resource = None;
                result
            })
        });
        let result_json = persisted_result
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        transaction.execute(
            "INSERT INTO resource_projections(
               path, parent_path, projection_kind, source_revision, valid_at,
               state_revision, status, rating, color_label, pick_label, result_json, error
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, NULL, ?8, ?9)
             ON CONFLICT(path, projection_kind) DO UPDATE SET
               parent_path=excluded.parent_path,
               source_revision=excluded.source_revision,
               valid_at=excluded.valid_at,
               state_revision=excluded.state_revision,
               status=excluded.status,
               rating=NULL,
               color_label=NULL,
               pick_label=NULL,
               result_json=excluded.result_json,
               error=excluded.error",
            params![
                candidate.path.to_string_lossy(),
                parent_string(&candidate.path),
                image_projection_kind(candidate.level),
                candidate.source_revision,
                candidate.valid_at as i64,
                candidate.state_revision as i64,
                status_name(candidate.status),
                result_json,
                candidate.error,
            ],
        )?;
        transaction.commit()?;
        Ok(candidate)
    }

    pub fn invalidate_resource_projections(&self, directory: &Path) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = next_resource_revision(&transaction)?;
        transaction.execute(
            "UPDATE resource_projections SET
               source_revision = 'invalidated:' || CAST(?1 AS TEXT),
               valid_at = ?1,
               state_revision = ?1,
               status = 'error',
               rating = NULL,
               color_label = NULL,
               pick_label = NULL,
               result_json = NULL,
               error = 'invalidated'
             WHERE parent_path = ?2",
            params![revision as i64, directory.to_string_lossy()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn invalidate_image_projections(&self) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = next_resource_revision(&transaction)?;
        transaction.execute(
            "UPDATE resource_projections SET
               source_revision = 'invalidated:' || CAST(?1 AS TEXT),
               valid_at = ?1,
               state_revision = ?1,
               status = 'error',
               result_json = NULL,
               error = 'invalidated'
             WHERE projection_kind LIKE 'image:%'",
            params![revision as i64],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Rebuilds one root in low-cost directory batches. Existing completed
    /// rows remain queryable until the final cleanup transaction, and only one
    /// worker per canonical root is admitted at a time.
    pub fn index_root(&self, root: &Path) -> Result<Option<IndexStats>, LibraryError> {
        self.index_root_with_progress(root, |_| {})
    }

    /// Returns whether a registered root has no complete index generation.
    /// Normal folder opens reuse a completed generation; explicit refresh
    /// invalidates it and makes this check true again.
    pub fn root_needs_index(&self, root: &Path) -> Result<bool, LibraryError> {
        let root = if self.roots()?.iter().any(|stored| stored == root) {
            root.to_owned()
        } else {
            root.canonicalize().unwrap_or_else(|_| root.to_owned())
        };
        let root = root.to_string_lossy();
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let registered = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM library_roots WHERE path = ?1)",
            params![root],
            |row| row.get::<_, bool>(0),
        )?;
        Ok(registered && !has_completed_index(&connection, &root)?)
    }

    pub fn index_root_with_progress(
        &self,
        root: &Path,
        mut report_progress: impl FnMut(IndexProgress),
    ) -> Result<Option<IndexStats>, LibraryError> {
        let root = root.canonicalize()?;
        {
            let mut active = self.indexing_roots.lock();
            if !active.insert(root.clone()) {
                return Ok(None);
            }
        }
        let _gate = self.index_gate.lock();
        let result = self.index_root_inner(&root, &mut report_progress);
        self.indexing_roots.lock().remove(&root);
        result.map(Some)
    }

    pub fn list_assets(
        &self,
        root: &Path,
        directory: &Path,
        query: &AssetQuery,
        offset: usize,
    ) -> Result<Option<Page<AssetSummary>>, LibraryError> {
        if query.minimum_rating.is_some() || !query.color_labels.is_empty() {
            return Ok(None);
        }
        let root = root.to_string_lossy();
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        if !has_completed_index(&connection, &root)? {
            return Ok(None);
        }
        let page_size = query
            .page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let kind = query.kind.map(kind_name);
        let direction = if matches!(query.direction, SortDirection::Descending) {
            "DESC"
        } else {
            "ASC"
        };
        let order = match query.sort {
            AssetSort::Name => "a.name COLLATE NOCASE",
            AssetSort::Modified => "a.modified_at_ms",
            AssetSort::Size => "a.size_bytes",
            AssetSort::Kind => "a.kind",
        };
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let (total, sql, search_expression) = if let Some(search) = search {
            let expression = fts_expression(search);
            let total = connection.query_row(
                "SELECT COUNT(*)
                 FROM indexed_assets a
                 JOIN indexed_asset_search s ON s.path = a.path AND s.root_path = a.root_path
                 WHERE s.root_path = ?1
                   AND indexed_asset_search MATCH ?2
                   AND (?3 IS NULL OR a.kind = ?3)",
                params![root, expression, kind],
                |row| row.get::<_, usize>(0),
            )?;
            (
                total,
                format!(
                    "SELECT a.id, a.path, a.name, a.extension, a.kind,
                            a.size_bytes, a.modified_at_ms, a.has_sidecar
                     FROM indexed_assets a
                     JOIN indexed_asset_search s ON s.path = a.path AND s.root_path = a.root_path
                     WHERE s.root_path = ?1
                       AND indexed_asset_search MATCH ?2
                       AND (?3 IS NULL OR a.kind = ?3)
                     ORDER BY {order} {direction}, a.name {direction}
                     LIMIT ?4 OFFSET ?5"
                ),
                Some(expression),
            )
        } else {
            let directory = directory.to_string_lossy();
            let total = connection.query_row(
                "SELECT COUNT(*) FROM indexed_assets a
                 WHERE a.root_path = ?1 AND a.parent_path = ?2
                   AND (?3 IS NULL OR a.kind = ?3)",
                params![root, directory, kind],
                |row| row.get::<_, usize>(0),
            )?;
            (
                total,
                format!(
                    "SELECT a.id, a.path, a.name, a.extension, a.kind,
                            a.size_bytes, a.modified_at_ms, a.has_sidecar
                     FROM indexed_assets a
                     WHERE a.root_path = ?1 AND a.parent_path = ?2
                       AND (?3 IS NULL OR a.kind = ?3)
                     ORDER BY {order} {direction}, a.name {direction}
                     LIMIT ?4 OFFSET ?5"
                ),
                None,
            )
        };
        let mut statement = connection.prepare(&sql)?;
        let items = if let Some(expression) = search_expression {
            statement
                .query_map(
                    params![root, expression, kind, page_size, offset],
                    asset_from_row,
                )?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            statement
                .query_map(
                    params![root, directory.to_string_lossy(), kind, page_size, offset],
                    asset_from_row,
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        let end = offset.saturating_add(items.len());
        Ok(Some(Page {
            items,
            next_cursor: (end < total).then_some(end),
            total,
        }))
    }

    pub fn list_directories(
        &self,
        root: &Path,
        parent: &Path,
    ) -> Result<Option<Vec<DirectorySummary>>, LibraryError> {
        let root = root.to_string_lossy();
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        if !has_completed_directory_index(&connection, &root)? {
            return Ok(None);
        }
        let mut statement = connection.prepare(
            "SELECT path, name, has_children FROM indexed_directories
             WHERE root_path = ?1 AND parent_path = ?2
             ORDER BY name COLLATE NOCASE, name",
        )?;
        let directories = statement
            .query_map(params![root, parent.to_string_lossy()], |row| {
                Ok(DirectorySummary {
                    path: PathBuf::from(row.get::<_, String>(0)?),
                    name: row.get(1)?,
                    has_children: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(directories))
    }

    pub fn search_directories(
        &self,
        root: &Path,
        search: &str,
    ) -> Result<Option<Vec<DirectorySearchMatch>>, LibraryError> {
        let search = search.trim();
        if search.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let root_text = root.to_string_lossy();
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        if !has_completed_directory_index(&connection, &root_text)? {
            return Ok(None);
        }
        let mut statement = connection.prepare(
            "SELECT path, name, has_children FROM indexed_directories
             WHERE root_path = ?1 AND name LIKE '%' || ?2 || '%' COLLATE NOCASE
             ORDER BY
               CASE
                 WHEN name = ?2 COLLATE NOCASE THEN 0
                 WHEN name LIKE ?2 || '%' COLLATE NOCASE THEN 1
                 ELSE 2
               END,
               name COLLATE NOCASE,
               path DESC",
        )?;
        let directories = statement
            .query_map(params![root_text, search], |row| {
                Ok(DirectorySummary {
                    path: PathBuf::from(row.get::<_, String>(0)?),
                    name: row.get(1)?,
                    has_children: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(
            directories
                .into_iter()
                .map(|directory| DirectorySearchMatch {
                    ancestors: directory_ancestors(root, &directory.path),
                    directory,
                })
                .collect(),
        ))
    }

    pub fn invalidate_index(&self, root: &Path) -> Result<(), LibraryError> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM indexed_roots WHERE root_path = ?1",
            params![root.to_string_lossy()],
        )?;
        transaction.execute(
            "DELETE FROM indexed_directory_roots WHERE root_path = ?1",
            params![root.to_string_lossy()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn index_root_inner(
        &self,
        root: &Path,
        report_progress: &mut impl FnMut(IndexProgress),
    ) -> Result<IndexStats, LibraryError> {
        let scan_id = self.next_scan_id()?;
        let mut queue = DirectoryPriorityQueue::new(root.to_owned());
        let mut discovered_directories = Vec::new();
        let mut asset_count = 0;
        let mut directory_count = 0;

        // Record the complete directory tree first. Besides making folder
        // discovery the highest-priority work, this avoids touching asset
        // metadata in a large directory until every folder is known.
        while let Some((directory, depth)) = queue.pop_next() {
            report_progress(IndexProgress {
                root_path: root.to_owned(),
                current_directory: directory.clone(),
                pending_directory_count: queue.pending.len(),
                asset_count,
                directory_count,
                stage: IndexStage::Directories,
            });
            if !self.contains_root(root)? {
                return Ok(IndexStats {
                    asset_count,
                    directory_count,
                });
            }
            // Preserve the last completed generation if any directory becomes
            // unreadable. Publishing a partial scan as complete would turn a
            // transient permission or volume error into false deletions.
            self.foreground.wait_for_background();
            let directories = oxy_fs::scan_index_directories_with_checkpoint(&directory, || {
                self.foreground.wait_for_background();
            })?;
            let safe_directories = queue.enqueue_children(depth, directories);
            directory_count += safe_directories.len();
            self.write_directory_index_batch(root, &directory, scan_id, &safe_directories)?;
            discovered_directories.push(directory.clone());
            report_progress(IndexProgress {
                root_path: root.to_owned(),
                current_directory: directory,
                pending_directory_count: queue.pending.len(),
                asset_count,
                directory_count,
                stage: IndexStage::Directories,
            });
        }

        if !self.finish_directory_index(root, scan_id, directory_count)? {
            return Ok(IndexStats {
                asset_count,
                directory_count,
            });
        }

        for (index, directory) in discovered_directories.iter().enumerate() {
            report_progress(IndexProgress {
                root_path: root.to_owned(),
                current_directory: directory.clone(),
                pending_directory_count: discovered_directories.len() - index,
                asset_count,
                directory_count,
                stage: IndexStage::Assets,
            });
            if !self.contains_root(root)? {
                return Ok(IndexStats {
                    asset_count,
                    directory_count,
                });
            }
            self.foreground.wait_for_background();
            let assets = oxy_fs::scan_assets_with_progress(directory, |_| {
                self.foreground.wait_for_background();
            })?;
            asset_count += assets.len();
            self.write_asset_index_batch(root, directory, scan_id, &assets)?;
            report_progress(IndexProgress {
                root_path: root.to_owned(),
                current_directory: directory.clone(),
                pending_directory_count: discovered_directories.len() - index - 1,
                asset_count,
                directory_count,
                stage: IndexStage::Assets,
            });
        }
        self.finish_index(root, scan_id, asset_count, directory_count)?;
        Ok(IndexStats {
            asset_count,
            directory_count,
        })
    }

    fn write_directory_index_batch(
        &self,
        root: &Path,
        parent: &Path,
        scan_id: i64,
        directories: &[DirectorySummary],
    ) -> Result<(), LibraryError> {
        {
            let mut connection = self.connection.lock();
            let transaction = connection.transaction()?;
            if !root_is_registered(&transaction, &root.to_string_lossy())? {
                return Ok(());
            }
            // The parent was inserted optimistically when its own parent was
            // scanned. Once this level has been visited, its child status is
            // authoritative and can be corrected without another filesystem read.
            transaction.execute(
                "UPDATE indexed_directories
                 SET has_children = ?3, scan_id = ?4
                 WHERE root_path = ?1 AND path = ?2",
                params![
                    root.to_string_lossy(),
                    parent.to_string_lossy(),
                    !directories.is_empty(),
                    scan_id,
                ],
            )?;
            transaction.commit()?;
            // Hand the writer to an existing waiter before starting another batch.
            MutexGuard::unlock_fair(connection);
        }

        let mut offset = 0;
        while offset < directories.len() {
            let batch =
                &directories[offset..directories.len().min(offset + INDEX_WRITE_BATCH_SIZE)];
            let mut connection = self.connection.lock();
            let started = Instant::now();
            let transaction = connection.transaction()?;
            if !root_is_registered(&transaction, &root.to_string_lossy())? {
                return Ok(());
            }
            {
                let mut statement = transaction.prepare_cached(
                    "INSERT INTO indexed_directories(
                       root_path, path, parent_path, name, has_children, scan_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(root_path, path) DO UPDATE SET
                       root_path=excluded.root_path, parent_path=excluded.parent_path,
                       name=excluded.name, has_children=excluded.has_children,
                       scan_id=excluded.scan_id",
                )?;
                for (processed, directory) in batch.iter().enumerate() {
                    if processed > 0 && started.elapsed() >= INDEX_WRITE_TIME_SLICE {
                        break;
                    }
                    offset += 1;
                    statement.execute(params![
                        root.to_string_lossy(),
                        directory.path.to_string_lossy(),
                        parent.to_string_lossy(),
                        directory.name,
                        directory.has_children,
                        scan_id,
                    ])?;
                }
            }
            transaction.commit()?;
            // Hand the writer to an existing waiter before starting another batch.
            MutexGuard::unlock_fair(connection);
        }
        Ok(())
    }

    fn write_asset_index_batch(
        &self,
        root: &Path,
        parent: &Path,
        scan_id: i64,
        assets: &[AssetSummary],
    ) -> Result<(), LibraryError> {
        let mut offset = 0;
        while offset < assets.len() {
            let batch = &assets[offset..assets.len().min(offset + INDEX_WRITE_BATCH_SIZE)];
            let mut connection = self.connection.lock();
            let started = Instant::now();
            let transaction = connection.transaction()?;
            if !root_is_registered(&transaction, &root.to_string_lossy())? {
                return Ok(());
            }
            {
                let mut mark_unchanged = transaction.prepare_cached(
                    "UPDATE indexed_assets SET scan_id = ?11
                     WHERE root_path = ?1 AND path = ?2 AND parent_path = ?3
                       AND id = ?4 AND name = ?5 AND extension = ?6 AND kind = ?7
                       AND modified_at_ms = ?8 AND size_bytes = ?9
                       AND has_sidecar = ?10",
                )?;
                let mut upsert = transaction.prepare_cached(
                    "INSERT INTO indexed_assets(
                       root_path, path, parent_path, id, name, extension, kind,
                       modified_at_ms, size_bytes, has_sidecar, scan_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(root_path, path) DO UPDATE SET
                       root_path=excluded.root_path, parent_path=excluded.parent_path,
                       id=excluded.id, name=excluded.name, extension=excluded.extension,
                       kind=excluded.kind, modified_at_ms=excluded.modified_at_ms,
                       size_bytes=excluded.size_bytes, has_sidecar=excluded.has_sidecar,
                       scan_id=excluded.scan_id",
                )?;
                let mut insert_search_key = transaction.prepare_cached(
                    "INSERT INTO indexed_asset_search_keys(root_path, path) VALUES (?1, ?2)
                     ON CONFLICT(root_path, path) DO NOTHING",
                )?;
                let mut search_key = transaction.prepare_cached(
                    "SELECT search_rowid FROM indexed_asset_search_keys
                     WHERE root_path = ?1 AND path = ?2",
                )?;
                let mut insert_search = transaction.prepare_cached(
                    "INSERT OR REPLACE INTO indexed_asset_search(rowid, path, root_path, name, directory)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                for (processed, asset) in batch.iter().enumerate() {
                    if processed > 0 && started.elapsed() >= INDEX_WRITE_TIME_SLICE {
                        break;
                    }
                    offset += 1;
                    let values = params![
                        root.to_string_lossy(),
                        asset.path.to_string_lossy(),
                        parent.to_string_lossy(),
                        asset.id,
                        asset.name,
                        asset.extension,
                        kind_name(asset.kind),
                        asset.modified_at_ms,
                        asset.size_bytes,
                        asset.has_sidecar,
                        scan_id,
                    ];
                    if mark_unchanged.execute(values)? > 0 {
                        continue;
                    }
                    upsert.execute(params![
                        root.to_string_lossy(),
                        asset.path.to_string_lossy(),
                        parent.to_string_lossy(),
                        asset.id,
                        asset.name,
                        asset.extension,
                        kind_name(asset.kind),
                        asset.modified_at_ms,
                        asset.size_bytes,
                        asset.has_sidecar,
                        scan_id,
                    ])?;
                    insert_search_key.execute(params![
                        root.to_string_lossy(),
                        asset.path.to_string_lossy()
                    ])?;
                    let search_rowid: i64 = search_key.query_row(
                        params![root.to_string_lossy(), asset.path.to_string_lossy()],
                        |row| row.get(0),
                    )?;
                    insert_search.execute(params![
                        search_rowid,
                        asset.path.to_string_lossy(),
                        root.to_string_lossy(),
                        asset.name,
                        parent.to_string_lossy(),
                    ])?;
                }
            }
            transaction.commit()?;
            // Hand the writer to an existing waiter before starting another batch.
            MutexGuard::unlock_fair(connection);
        }
        Ok(())
    }

    fn next_scan_id(&self) -> Result<i64, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let scan_id = transaction.query_row(
            "SELECT next_scan_id FROM library_index_sequence WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        transaction.execute(
            "UPDATE library_index_sequence SET next_scan_id = next_scan_id + 1 WHERE id = 1",
            [],
        )?;
        transaction.commit()?;
        Ok(scan_id)
    }

    fn finish_directory_index(
        &self,
        root: &Path,
        scan_id: i64,
        directory_count: usize,
    ) -> Result<bool, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        if !root_is_registered(&transaction, &root.to_string_lossy())? {
            return Ok(false);
        }
        transaction.execute(
            "DELETE FROM indexed_directories WHERE root_path = ?1 AND scan_id != ?2",
            params![root.to_string_lossy(), scan_id],
        )?;
        transaction.execute(
            "INSERT INTO indexed_directory_roots(root_path, indexed_at, directory_count)
             VALUES (?1, unixepoch(), ?2)
             ON CONFLICT(root_path) DO UPDATE SET
               indexed_at=excluded.indexed_at,
               directory_count=excluded.directory_count",
            params![root.to_string_lossy(), directory_count],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    fn finish_index(
        &self,
        root: &Path,
        scan_id: i64,
        asset_count: usize,
        directory_count: usize,
    ) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        if !root_is_registered(&transaction, &root.to_string_lossy())? {
            return Ok(());
        }
        transaction.execute(
            "DELETE FROM indexed_asset_search WHERE rowid IN (
               SELECT k.search_rowid FROM indexed_asset_search_keys k
               JOIN indexed_assets a ON a.root_path = k.root_path AND a.path = k.path
               WHERE a.root_path = ?1 AND a.scan_id != ?2
             )",
            params![root.to_string_lossy(), scan_id],
        )?;
        transaction.execute(
            "DELETE FROM indexed_asset_search_keys WHERE root_path = ?1 AND path IN (
               SELECT path FROM indexed_assets WHERE root_path = ?1 AND scan_id != ?2
             )",
            params![root.to_string_lossy(), scan_id],
        )?;
        transaction.execute(
            "DELETE FROM indexed_assets WHERE root_path = ?1 AND scan_id != ?2",
            params![root.to_string_lossy(), scan_id],
        )?;
        transaction.execute(
            "INSERT INTO indexed_roots(root_path, indexed_at, asset_count, directory_count)
             VALUES (?1, unixepoch(), ?2, ?3)
             ON CONFLICT(root_path) DO UPDATE SET
               indexed_at=excluded.indexed_at,
               asset_count=excluded.asset_count,
               directory_count=excluded.directory_count",
            params![root.to_string_lossy(), asset_count, directory_count],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn next_resource_revision(transaction: &Transaction<'_>) -> Result<u64, rusqlite::Error> {
    let revision = transaction.query_row(
        "SELECT next_revision FROM resource_projection_sequence WHERE id = 1",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    transaction.execute(
        "UPDATE resource_projection_sequence SET next_revision = next_revision + 1 WHERE id = 1",
        [],
    )?;
    Ok(revision.max(0) as u64)
}

fn read_metadata_projection(
    connection: &Connection,
    path: &Path,
) -> Result<Option<MetadataProjection>, LibraryError> {
    let row = connection
        .query_row(
            "SELECT source_revision, state_revision, valid_at, status,
                    rating, color_label, pick_label, error
             FROM resource_projections
             WHERE path = ?1 AND projection_kind = 'metadata'",
            params![path.to_string_lossy()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<u8>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(
            source_revision,
            state_revision,
            valid_at,
            status,
            rating,
            color_label,
            pick_label,
            error,
        )| {
            Ok(MetadataProjection {
                path: path.to_path_buf(),
                source_revision,
                state_revision: state_revision.max(0) as u64,
                valid_at: valid_at.max(0) as u64,
                status: parse_status(&status)?,
                rating,
                color_label,
                pick_label: parse_pick_label(pick_label)?,
                error,
            })
        },
    )
    .transpose()
}

fn read_image_projection(
    connection: &Connection,
    path: &Path,
    level: RenderLevel,
) -> Result<Option<ImageProjection>, LibraryError> {
    let row = connection
        .query_row(
            "SELECT source_revision, state_revision, valid_at, status,
                    result_json, error
             FROM resource_projections
             WHERE path = ?1 AND projection_kind = ?2",
            params![path.to_string_lossy(), image_projection_kind(level)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(source_revision, state_revision, valid_at, status, result_json, error)| {
            Ok(ImageProjection {
                path: path.to_path_buf(),
                source_revision,
                state_revision: state_revision.max(0) as u64,
                valid_at: valid_at.max(0) as u64,
                status: parse_status(&status)?,
                level,
                result: result_json
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?,
                error,
            })
        },
    )
    .transpose()
}

fn parent_string(path: &Path) -> String {
    path.parent()
        .unwrap_or_else(|| Path::new(""))
        .to_string_lossy()
        .into_owned()
}

fn image_projection_kind(level: RenderLevel) -> &'static str {
    match level {
        RenderLevel::Thumbnail => "image:thumbnail",
        RenderLevel::Preview => "image:preview",
        RenderLevel::Full => "image:full",
    }
}

fn status_name(status: ResourceLoadStatus) -> &'static str {
    match status {
        ResourceLoadStatus::Loading => "loading",
        ResourceLoadStatus::Ready => "ready",
        ResourceLoadStatus::Error => "error",
    }
}

fn parse_status(status: &str) -> Result<ResourceLoadStatus, rusqlite::Error> {
    match status {
        "loading" => Ok(ResourceLoadStatus::Loading),
        "ready" => Ok(ResourceLoadStatus::Ready),
        "error" => Ok(ResourceLoadStatus::Error),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn has_completed_index(connection: &Connection, root: &str) -> Result<bool, rusqlite::Error> {
    connection
        .query_row(
            "SELECT 1 FROM indexed_roots WHERE root_path = ?1",
            params![root],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
}

fn has_completed_directory_index(
    connection: &Connection,
    root: &str,
) -> Result<bool, rusqlite::Error> {
    connection
        .query_row(
            "SELECT 1 FROM indexed_directory_roots WHERE root_path = ?1",
            params![root],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
}

fn root_is_registered(transaction: &Transaction<'_>, root: &str) -> Result<bool, rusqlite::Error> {
    transaction
        .query_row(
            "SELECT 1 FROM library_roots WHERE path = ?1",
            params![root],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
}

fn fts_expression(search: &str) -> String {
    search
        .split_whitespace()
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn directory_ancestors(root: &Path, directory: &Path) -> Vec<DirectorySummary> {
    let Ok(relative) = directory.strip_prefix(root) else {
        return Vec::new();
    };
    let components = relative.components().collect::<Vec<_>>();
    let mut path = root.to_path_buf();
    components
        .iter()
        .take(components.len().saturating_sub(1))
        .map(|component| {
            path.push(component.as_os_str());
            DirectorySummary {
                path: path.clone(),
                name: component.as_os_str().to_string_lossy().into_owned(),
                has_children: true,
            }
        })
        .collect()
}

fn kind_name(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Raw => "raw",
        AssetKind::Jpeg => "jpeg",
        AssetKind::Heif => "heif",
        AssetKind::Png => "png",
        AssetKind::Tiff => "tiff",
        AssetKind::Webp => "webp",
    }
}

fn parse_kind(value: &str) -> Result<AssetKind, rusqlite::Error> {
    match value {
        "raw" => Ok(AssetKind::Raw),
        "jpeg" => Ok(AssetKind::Jpeg),
        "heif" => Ok(AssetKind::Heif),
        "png" => Ok(AssetKind::Png),
        "tiff" => Ok(AssetKind::Tiff),
        "webp" => Ok(AssetKind::Webp),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn pick_label_name(value: PickLabel) -> &'static str {
    match value {
        PickLabel::Rejected => "rejected",
        PickLabel::Pending => "pending",
        PickLabel::Accepted => "accepted",
    }
}

fn parse_pick_label(value: Option<String>) -> Result<Option<PickLabel>, rusqlite::Error> {
    value
        .map(|value| match value.as_str() {
            "rejected" => Ok(PickLabel::Rejected),
            "pending" => Ok(PickLabel::Pending),
            "accepted" => Ok(PickLabel::Accepted),
            _ => Err(rusqlite::Error::InvalidQuery),
        })
        .transpose()
}

fn asset_from_row(row: &rusqlite::Row<'_>) -> Result<AssetSummary, rusqlite::Error> {
    Ok(AssetSummary {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        name: row.get(2)?,
        extension: row.get(3)?,
        kind: parse_kind(&row.get::<_, String>(4)?)?,
        size_bytes: row.get(5)?,
        modified_at_ms: row.get(6)?,
        has_sidecar: row.get(7)?,
        rating: None,
        color_label: None,
        pick_label: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn query(search: Option<&str>) -> AssetQuery {
        AssetQuery {
            search: search.map(str::to_owned),
            sort: AssetSort::Name,
            direction: SortDirection::Ascending,
            page_size: Some(10),
            ..AssetQuery::default()
        }
    }

    #[test]
    fn stores_only_explicit_roots() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        library.add_root(root.path()).unwrap();
        assert_eq!(
            library.roots().unwrap(),
            vec![root.path().canonicalize().unwrap()]
        );
    }

    #[test]
    fn preserves_import_order_and_allows_explicit_reordering() {
        let library = Library::in_memory().unwrap();
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        let third = tempdir().unwrap();
        library.add_root(second.path()).unwrap();
        library.add_root(first.path()).unwrap();
        library.add_root(third.path()).unwrap();

        let expected =
            [third.path(), second.path(), first.path()].map(|path| path.canonicalize().unwrap());
        library.reorder_roots(&expected).unwrap();
        assert_eq!(library.roots().unwrap(), expected);
    }

    #[test]
    fn rejects_incomplete_root_orders() {
        let library = Library::in_memory().unwrap();
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        library.add_root(first.path()).unwrap();
        library.add_root(second.path()).unwrap();

        assert!(matches!(
            library.reorder_roots(&[first.path().canonicalize().unwrap()]),
            Err(LibraryError::InvalidRootOrder)
        ));
    }

    #[test]
    fn migrates_legacy_roots_and_persists_reordering() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("library.sqlite");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        {
            let legacy = Connection::open(&database).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE library_roots (
                       path TEXT PRIMARY KEY NOT NULL,
                       added_at INTEGER NOT NULL DEFAULT (unixepoch())
                     );",
                )
                .unwrap();
            legacy
                .execute(
                    "INSERT INTO library_roots(path, added_at) VALUES (?1, 1)",
                    params![first.to_string_lossy()],
                )
                .unwrap();
            legacy
                .execute(
                    "INSERT INTO library_roots(path, added_at) VALUES (?1, 2)",
                    params![second.to_string_lossy()],
                )
                .unwrap();
        }

        let library = Library::open(&database).unwrap();
        assert_eq!(library.roots().unwrap(), [first.clone(), second.clone()]);
        library
            .reorder_roots(&[second.clone(), first.clone()])
            .unwrap();
        drop(library);

        let reopened = Library::open(&database).unwrap();
        assert_eq!(reopened.roots().unwrap(), [second, first]);
    }

    #[test]
    fn migrates_resource_projection_state_revision_and_pick_labels() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("library.sqlite");
        {
            let legacy = Connection::open(&database).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE resource_projections (
                       path TEXT NOT NULL,
                       parent_path TEXT NOT NULL,
                       projection_kind TEXT NOT NULL,
                       source_revision TEXT NOT NULL,
                       valid_at INTEGER NOT NULL,
                       projection_revision INTEGER NOT NULL,
                       status TEXT NOT NULL,
                       rating INTEGER,
                       color_label TEXT,
                       result_json TEXT,
                       error TEXT,
                       PRIMARY KEY(path, projection_kind)
                     );
                     INSERT INTO resource_projections(
                       path, parent_path, projection_kind, source_revision, valid_at,
                       projection_revision, status, rating, color_label, result_json, error
                     ) VALUES (
                       'legacy.jpg', '', 'metadata', 'legacy-source', 7,
                       11, 'ready', 4, 'Red', NULL, NULL
                     );",
                )
                .unwrap();
        }

        let library = Library::open(&database).unwrap();
        let columns = library
            .connection
            .lock()
            .prepare("PRAGMA table_info(resource_projections)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "pick_label"));
        assert!(columns.iter().any(|column| column == "state_revision"));
        assert!(!columns.iter().any(|column| column == "projection_revision"));
        let migrated = library
            .metadata_projection(Path::new("legacy.jpg"), "legacy-source")
            .unwrap()
            .unwrap();
        assert_eq!(migrated.state_revision, 11);
        assert_eq!(migrated.rating, Some(4));
    }

    #[test]
    fn directory_priority_queue_visits_all_siblings_before_descendants() {
        let root = tempdir().unwrap();
        for path in ["beta/deep", "alpha/deep"] {
            std::fs::create_dir_all(root.path().join(path)).unwrap();
        }
        let canonical_root = root.path().canonicalize().unwrap();
        let mut queue = DirectoryPriorityQueue::new(canonical_root.clone());
        let mut order = Vec::new();

        while let Some((directory, depth)) = queue.pop_next() {
            order.push(directory.strip_prefix(&canonical_root).unwrap().to_owned());
            let scan = oxy_fs::scan_index_directory(&directory).unwrap();
            queue.enqueue_children(depth, scan.directories);
        }

        assert_eq!(
            order,
            ["", "alpha", "beta", "alpha/deep", "beta/deep"].map(PathBuf::from)
        );
    }

    #[test]
    fn higher_level_directory_preempts_an_earlier_deep_directory() {
        let root = tempdir().unwrap();
        let shallow = root.path().join("shallow");
        let deep = root.path().join("parent").join("deep");
        std::fs::create_dir_all(&shallow).unwrap();
        std::fs::create_dir_all(&deep).unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        let mut queue = DirectoryPriorityQueue::new(canonical_root);
        queue.pop_next();

        queue.enqueue_children(
            1,
            vec![DirectorySummary {
                path: deep.canonicalize().unwrap(),
                name: "deep".into(),
                has_children: false,
            }],
        );
        queue.enqueue_children(
            0,
            vec![DirectorySummary {
                path: shallow.canonicalize().unwrap(),
                name: "shallow".into(),
                has_children: false,
            }],
        );

        assert_eq!(queue.pop_next(), Some((shallow.canonicalize().unwrap(), 1)));
        assert_eq!(queue.pop_next(), Some((deep.canonicalize().unwrap(), 2)));
    }

    #[test]
    fn stores_parent_and_child_as_independent_roots_and_removes_one() {
        let library = Library::in_memory().unwrap();
        let parent = tempdir().unwrap();
        let child = parent.path().join("child");
        std::fs::create_dir(&child).unwrap();

        library.add_root(parent.path()).unwrap();
        library.add_root(&child).unwrap();
        assert_eq!(library.roots().unwrap().len(), 2);

        library.remove_root(&child).unwrap();
        assert_eq!(
            library.roots().unwrap(),
            vec![parent.path().canonicalize().unwrap()]
        );
    }

    #[test]
    fn indexes_directory_batches_and_searches_names_and_folder_paths() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let coast = root.path().join("Trips").join("Coast");
        std::fs::create_dir_all(&coast).unwrap();
        std::fs::write(root.path().join("portrait.jpg"), b"jpeg").unwrap();
        std::fs::write(coast.join("sunrise.ARW"), b"raw").unwrap();
        library.add_root(root.path()).unwrap();

        let stats = library.index_root(root.path()).unwrap().unwrap();

        assert_eq!(stats.asset_count, 2);
        assert_eq!(stats.directory_count, 2);
        let canonical_root = root.path().canonicalize().unwrap();
        let root_page = library
            .list_assets(&canonical_root, &canonical_root, &query(None), 0)
            .unwrap()
            .unwrap();
        assert_eq!(root_page.items.len(), 1);
        assert_eq!(root_page.items[0].name, "portrait.jpg");

        let name_results = library
            .list_assets(&canonical_root, &canonical_root, &query(Some("sun")), 0)
            .unwrap()
            .unwrap();
        assert_eq!(name_results.items[0].name, "sunrise.ARW");
        let folder_results = library
            .list_assets(&canonical_root, &canonical_root, &query(Some("Coast")), 0)
            .unwrap()
            .unwrap();
        assert_eq!(folder_results.items[0].name, "sunrise.ARW");
        let punctuation_results = library
            .list_assets(&canonical_root, &canonical_root, &query(Some("\"")), 0)
            .unwrap()
            .unwrap();
        assert_eq!(punctuation_results.total, 0);

        let directories = library
            .list_directories(&canonical_root, &canonical_root)
            .unwrap()
            .unwrap();
        assert_eq!(directories[0].name, "Trips");
        assert!(directories[0].has_children);

        let directory_matches = library
            .search_directories(&canonical_root, "coa")
            .unwrap()
            .unwrap();
        assert_eq!(directory_matches.len(), 1);
        assert_eq!(directory_matches[0].directory.name, "Coast");
        assert!(!directory_matches[0].directory.has_children);
        assert_eq!(directory_matches[0].ancestors.len(), 1);
        assert_eq!(directory_matches[0].ancestors[0].name, "Trips");
        assert!(
            library
                .search_directories(&canonical_root, "Trips/Coast")
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn completed_index_is_reused_until_explicitly_invalidated() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        library.add_root(root.path()).unwrap();

        assert!(library.root_needs_index(root.path()).unwrap());
        library.index_root(root.path()).unwrap();
        assert!(!library.root_needs_index(root.path()).unwrap());

        library.invalidate_index(root.path()).unwrap();
        assert!(library.root_needs_index(root.path()).unwrap());
    }

    #[test]
    fn wal_reads_finish_during_an_uncommitted_index_write() {
        let state = tempdir().unwrap();
        let library = Library::open(&state.path().join("library.sqlite")).unwrap();
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::write(root.join("photo.jpg"), b"jpeg").unwrap();
        library.add_root(&root).unwrap();
        library.index_root(&root).unwrap();

        let (sender, receiver) = std::sync::mpsc::channel();
        let completed = std::thread::scope(|scope| {
            let mut writer = library.connection.lock();
            let transaction = writer.transaction().unwrap();
            transaction
                .execute("UPDATE indexed_assets SET name = 'uncommitted.jpg'", [])
                .unwrap();
            scope.spawn(|| {
                let page = library
                    .list_assets(&root, &root, &query(None), 0)
                    .unwrap()
                    .unwrap();
                assert_eq!(page.items[0].name, "photo.jpg");
                assert!(library.contains_root(&root).unwrap());
                assert!(!library.root_needs_index(&root).unwrap());
                assert_eq!(library.roots().unwrap(), vec![root.clone()]);
                assert!(library.list_directories(&root, &root).unwrap().is_some());
                assert!(
                    library
                        .search_directories(&root, "photo")
                        .unwrap()
                        .is_some()
                );
                assert!(library.custom_tags().unwrap().is_empty());
                sender.send(()).unwrap();
            });
            let completed = receiver.recv_timeout(Duration::from_secs(2));
            // Always release before joining, so a lock regression fails instead of hanging.
            transaction.commit().unwrap();
            drop(writer);
            completed
        });
        assert!(completed.is_ok(), "WAL reader waited for the index writer");
        let page = library
            .list_assets(&root, &root, &query(None), 0)
            .unwrap()
            .unwrap();
        assert_eq!(page.items[0].name, "uncommitted.jpg");
    }

    #[test]
    fn projection_reads_do_not_wait_for_browsing_or_the_writer() {
        let state = tempdir().unwrap();
        let library = Library::open(&state.path().join("library.sqlite")).unwrap();
        let path = state.path().join("photo.jpg");
        let valid_at = library.next_resource_revision().unwrap();
        library
            .accept_metadata_projection(MetadataProjection {
                path: path.clone(),
                source_revision: "100:200:xmp-a".into(),
                state_revision: 0,
                valid_at,
                status: ResourceLoadStatus::Ready,
                rating: Some(4),
                color_label: None,
                pick_label: None,
                error: None,
            })
            .unwrap();
        let writer = library.connection.lock();
        let browsing = library.read_connection();
        let (sender, receiver) = std::sync::mpsc::channel();
        let completed = std::thread::scope(|scope| {
            scope.spawn(|| {
                assert_eq!(
                    library
                        .metadata_projection(&path, "100:200:xmp-a")
                        .unwrap()
                        .unwrap()
                        .rating,
                    Some(4)
                );
                assert!(
                    library
                        .image_projection(&path, RenderLevel::Thumbnail, "missing")
                        .unwrap()
                        .is_none()
                );
                sender.send(()).unwrap();
            });
            let completed = receiver.recv_timeout(Duration::from_secs(2));
            drop(browsing);
            drop(writer);
            completed
        });
        assert!(
            completed.is_ok(),
            "projection reader waited for browsing or indexing"
        );
    }

    #[test]
    #[ignore = "manual WAL contention benchmark; run with --ignored --nocapture"]
    fn wal_interaction_latency_during_large_index() {
        let state = tempdir().unwrap();
        let library = Library::open(&state.path().join("library.sqlite")).unwrap();
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::write(root.join("visible.jpg"), b"jpeg").unwrap();
        library.add_root(&root).unwrap();
        library.index_root(&root).unwrap();
        let template = oxy_fs::scan_index_assets(&root).unwrap().remove(0);
        let background = root.join("background");
        let assets: Vec<_> = (0..50_000)
            .map(|index| {
                let mut asset = template.clone();
                asset.name = format!("photo-{index}.jpg");
                asset.path = background.join(&asset.name);
                asset.id = format!("asset-{index}");
                asset
            })
            .collect();
        let scan_id = library.next_scan_id().unwrap();
        let start = std::sync::Barrier::new(2);
        let mut reads = Vec::new();
        let mut writes = Vec::new();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                start.wait();
                library
                    .write_asset_index_batch(&root, &background, scan_id, &assets)
                    .unwrap();
            });
            start.wait();
            for _ in 0..200 {
                let started = Instant::now();
                let page = library
                    .list_assets(&root, &root, &query(None), 0)
                    .unwrap()
                    .unwrap();
                assert_eq!(page.items.len(), 1);
                library
                    .metadata_projection(&template.path, "missing")
                    .unwrap();
                library
                    .image_projection(&template.path, RenderLevel::Thumbnail, "missing")
                    .unwrap();
                reads.push(started.elapsed());
                let started = Instant::now();
                library.next_resource_revision().unwrap();
                writes.push(started.elapsed());
            }
        });
        reads.sort_unstable();
        writes.sort_unstable();
        println!(
            "50k background writes, 200 foreground samples: read P95 {:?}, max {:?}; revision write P95 {:?}, max {:?}",
            reads[189], reads[199], writes[189], writes[199]
        );
    }

    #[test]
    #[ignore = "manual large-library indexing benchmark; run with --ignored --nocapture"]
    fn large_library_index_batch_latency() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        library.add_root(&root).unwrap();
        std::fs::write(root.join("template.jpg"), b"jpeg").unwrap();
        let template = oxy_fs::scan_index_assets(&root).unwrap().remove(0);
        let scan_id = library.next_scan_id().unwrap();
        let mut inserted = 0;
        for target in [1_000, 10_000, 50_000] {
            while inserted < target {
                let count = INDEX_WRITE_BATCH_SIZE.min(target - inserted);
                let assets: Vec<_> = (inserted..inserted + count)
                    .map(|index| {
                        let mut asset = template.clone();
                        asset.name = format!("photo-{index}.jpg");
                        asset.path = root.join(&asset.name);
                        asset.id = format!("asset-{index}");
                        asset
                    })
                    .collect();
                library
                    .write_asset_index_batch(&root, &root, scan_id, &assets)
                    .unwrap();
                inserted += count;
            }
            let assets: Vec<_> = (inserted..inserted + INDEX_WRITE_BATCH_SIZE)
                .map(|index| {
                    let mut asset = template.clone();
                    asset.name = format!("photo-{index}.jpg");
                    asset.path = root.join(&asset.name);
                    asset.id = format!("asset-{index}");
                    asset
                })
                .collect();
            let start = std::time::Instant::now();
            library
                .write_asset_index_batch(&root, &root, scan_id, &assets)
                .unwrap();
            println!(
                "{target} indexed assets: next {INDEX_WRITE_BATCH_SIZE} writes took {:?}",
                start.elapsed()
            );
            inserted += assets.len();
        }
        let connection = library.connection.lock();
        let count: usize = connection
            .query_row("SELECT COUNT(*) FROM indexed_asset_search", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, inserted);
    }

    #[test]
    fn migrates_search_keys_and_replaces_changed_terms_without_duplicates() {
        let state = tempdir().unwrap();
        let database = state.path().join("library.sqlite");
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::write(root.join("photo.jpg"), b"jpeg").unwrap();
        let library = Library::open(&database).unwrap();
        library.add_root(&root).unwrap();
        library.index_root(&root).unwrap();
        // Simulate the pre-migration database, including a non-default FTS rowid.
        library
            .connection
            .lock()
            .execute_batch(
                "UPDATE indexed_asset_search SET rowid = 1234;
             DROP TABLE indexed_asset_search_keys;",
            )
            .unwrap();
        drop(library);

        let library = Library::open(&database).unwrap();
        let mut assets = oxy_fs::scan_index_assets(&root).unwrap();
        assets[0].name = "replacement.jpg".into();
        let scan_id = library.next_scan_id().unwrap();
        library
            .write_asset_index_batch(&root, &root, scan_id, &assets)
            .unwrap();
        library.finish_index(&root, scan_id, 1, 0).unwrap();
        assert_eq!(
            library
                .list_assets(&root, &root, &query(Some("photo")), 0)
                .unwrap()
                .unwrap()
                .total,
            0
        );
        assert_eq!(
            library
                .list_assets(&root, &root, &query(Some("replacement")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
        let connection = library.connection.lock();
        let (count, rowid): (usize, i64) = connection
            .query_row(
                "SELECT COUNT(*), MIN(rowid) FROM indexed_asset_search",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((count, rowid), (1, 1234));
        drop(connection);
        drop(library);
        // Reopening must preserve the migrated key and searchable result.
        let library = Library::open(&database).unwrap();
        assert_eq!(
            library
                .list_assets(&root, &root, &query(Some("replacement")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
    }

    #[test]
    fn search_keys_keep_overlapping_roots_independent_and_clean_up() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let child = root.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("shared.jpg"), b"jpeg").unwrap();
        library.add_root(root.path()).unwrap();
        library.add_root(&child).unwrap();
        library.index_root(root.path()).unwrap();
        library.index_root(&child).unwrap();
        library.remove_root(root.path()).unwrap();
        let child = child.canonicalize().unwrap();
        assert_eq!(
            library
                .list_assets(&child, &child, &query(Some("shared")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
        std::fs::remove_file(child.join("shared.jpg")).unwrap();
        library.index_root(&child).unwrap();
        let connection = library.connection.lock();
        for table in ["indexed_asset_search", "indexed_asset_search_keys"] {
            let count: usize = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "stale rows in {table}");
        }
    }

    #[test]
    fn batched_reindex_updates_fts_only_to_the_current_asset_set() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        library.add_root(root.path()).unwrap();
        for index in 0..=INDEX_WRITE_BATCH_SIZE {
            std::fs::write(root.path().join(format!("old-{index}.jpg")), b"jpeg").unwrap();
        }
        library.index_root(root.path()).unwrap();

        for index in 0..INDEX_WRITE_BATCH_SIZE {
            std::fs::remove_file(root.path().join(format!("old-{index}.jpg"))).unwrap();
        }
        std::fs::write(root.path().join("new-result.jpg"), b"jpeg").unwrap();
        library.index_root(root.path()).unwrap();

        let canonical_root = root.path().canonicalize().unwrap();
        assert_eq!(
            library
                .list_assets(&canonical_root, &canonical_root, &query(Some("old")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            library
                .list_assets(&canonical_root, &canonical_root, &query(Some("new")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
        let search_rows = library
            .connection
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM indexed_asset_search WHERE root_path = ?1",
                params![canonical_root.to_string_lossy()],
                |row| row.get::<_, usize>(0),
            )
            .unwrap();
        assert_eq!(search_rows, 2);
    }

    #[test]
    fn reports_index_progress_for_each_directory_batch() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let child = root.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(root.path().join("root.jpg"), b"jpeg").unwrap();
        std::fs::write(child.join("nested.jpg"), b"jpeg").unwrap();
        library.add_root(root.path()).unwrap();
        let mut progress = Vec::new();

        library
            .index_root_with_progress(root.path(), |update| progress.push(update))
            .unwrap();

        let canonical_root = root.path().canonicalize().unwrap();
        assert_eq!(progress.first().unwrap().current_directory, canonical_root);
        assert_eq!(progress.first().unwrap().asset_count, 0);
        assert!(progress.iter().any(|update| {
            update.current_directory == child.canonicalize().unwrap()
                && update.pending_directory_count == 0
        }));
        let completed = progress.last().unwrap();
        assert_eq!(completed.asset_count, 2);
        assert_eq!(completed.directory_count, 1);
        assert_eq!(completed.pending_directory_count, 0);
    }

    #[test]
    fn records_the_complete_directory_tree_before_any_assets() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let nested = root.path().join("parent").join("child");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.path().join("root.jpg"), b"jpeg").unwrap();
        std::fs::write(nested.join("nested.jpg"), b"jpeg").unwrap();
        library.add_root(root.path()).unwrap();
        let mut observed_counts = Vec::new();

        library
            .index_root_with_progress(root.path(), |_| {
                let connection = library.connection.lock();
                let directory_rows = connection
                    .query_row("SELECT COUNT(*) FROM indexed_directories", [], |row| {
                        row.get::<_, usize>(0)
                    })
                    .unwrap();
                let asset_rows = connection
                    .query_row("SELECT COUNT(*) FROM indexed_assets", [], |row| {
                        row.get::<_, usize>(0)
                    })
                    .unwrap();
                observed_counts.push((directory_rows, asset_rows));
            })
            .unwrap();

        assert!(observed_counts.contains(&(2, 0)));
        assert!(
            observed_counts
                .iter()
                .filter(|(_, asset_rows)| *asset_rows > 0)
                .all(|(directory_rows, _)| *directory_rows == 2)
        );
    }

    #[test]
    fn directory_search_is_available_before_asset_indexing_finishes() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let matching = root.path().join("matching-folder");
        std::fs::create_dir(&matching).unwrap();
        std::fs::write(root.path().join("root.jpg"), b"jpeg").unwrap();
        library.add_root(root.path()).unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        let mut matches_during_asset_index = None;
        let mut assets_during_asset_index = None;

        library
            .index_root_with_progress(&canonical_root, |progress| {
                if progress.stage == IndexStage::Assets && matches_during_asset_index.is_none() {
                    matches_during_asset_index = Some(
                        library
                            .search_directories(&canonical_root, "matching")
                            .unwrap(),
                    );
                    assets_during_asset_index = Some(
                        library
                            .list_assets(&canonical_root, &canonical_root, &query(None), 0)
                            .unwrap(),
                    );
                }
            })
            .unwrap();

        let matches = matches_during_asset_index.unwrap().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].directory.path, matching.canonicalize().unwrap());
        assert!(matches!(assets_during_asset_index, Some(None)));
    }

    #[test]
    fn index_is_rebuildable_and_removed_with_its_root() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let asset = root.path().join("first.jpg");
        std::fs::write(&asset, b"jpeg").unwrap();
        library.add_root(root.path()).unwrap();
        library.index_root(root.path()).unwrap();
        std::fs::remove_file(asset).unwrap();

        library.index_root(root.path()).unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        let page = library
            .list_assets(&canonical_root, &canonical_root, &query(None), 0)
            .unwrap()
            .unwrap();
        assert_eq!(page.total, 0);

        library.remove_root(&canonical_root).unwrap();
        assert!(
            library
                .list_assets(&canonical_root, &canonical_root, &query(None), 0)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn parent_and_child_roots_keep_independent_index_rows() {
        let library = Library::in_memory().unwrap();
        let parent = tempdir().unwrap();
        let child = parent.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("shared.jpg"), b"jpeg").unwrap();
        library.add_root(parent.path()).unwrap();
        library.add_root(&child).unwrap();
        library.index_root(parent.path()).unwrap();
        library.index_root(&child).unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let child = child.canonicalize().unwrap();

        assert_eq!(
            library
                .list_assets(&parent, &parent, &query(Some("shared")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            library
                .list_assets(&child, &child, &query(None), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
    }

    #[test]
    fn scan_generation_survives_database_reopen() {
        let state = tempdir().unwrap();
        let database = state.path().join("library.sqlite");
        let root = tempdir().unwrap();
        let asset = root.path().join("stale.jpg");
        std::fs::write(&asset, b"jpeg").unwrap();
        {
            let library = Library::open(&database).unwrap();
            library.add_root(root.path()).unwrap();
            library.index_root(root.path()).unwrap();
        }
        std::fs::remove_file(asset).unwrap();

        let library = Library::open(&database).unwrap();
        library.index_root(root.path()).unwrap();
        let root = root.path().canonicalize().unwrap();
        let page = library
            .list_assets(&root, &root, &query(None), 0)
            .unwrap()
            .unwrap();
        assert_eq!(page.total, 0);
    }

    #[test]
    fn metadata_projection_survives_database_reopen() {
        let state = tempdir().unwrap();
        let database = state.path().join("library.sqlite");
        let path = state.path().join("rated.HIF");
        let source_revision = "100:200:xmp-a";
        {
            let library = Library::open(&database).unwrap();
            let valid_at = library.next_resource_revision().unwrap();
            let accepted = library
                .accept_metadata_projection(MetadataProjection {
                    path: path.clone(),
                    source_revision: source_revision.into(),
                    state_revision: 0,
                    valid_at,
                    status: ResourceLoadStatus::Ready,
                    rating: Some(4),
                    color_label: Some("Red".into()),
                    pick_label: Some(PickLabel::Accepted),
                    error: None,
                })
                .unwrap();
            assert!(accepted.state_revision > valid_at);
        }

        let reopened = Library::open(&database).unwrap();
        let cached = reopened
            .metadata_projection(&path, source_revision)
            .unwrap()
            .unwrap();
        assert_eq!(cached.rating, Some(4));
        assert_eq!(cached.color_label.as_deref(), Some("Red"));
        assert_eq!(cached.pick_label, Some(PickLabel::Accepted));
    }

    #[test]
    fn cheap_asset_stat_can_publish_a_snapshot_before_full_revision_validation() {
        let library = Library::in_memory().unwrap();
        let path = PathBuf::from("C:/photos/rated.HIF");
        let asset = AssetSummary {
            id: "rated".into(),
            path: path.clone(),
            name: "rated.HIF".into(),
            extension: "HIF".into(),
            kind: AssetKind::Heif,
            size_bytes: 200,
            modified_at_ms: 100,
            has_sidecar: false,
            rating: None,
            color_label: None,
            pick_label: None,
        };
        let valid_at = library.next_resource_revision().unwrap();
        library
            .accept_metadata_projection(MetadataProjection {
                path,
                source_revision: "100:200:0:0:embedded-xmp-digest".into(),
                state_revision: 0,
                valid_at,
                status: ResourceLoadStatus::Ready,
                rating: Some(3),
                color_label: Some("Yellow".into()),
                pick_label: None,
                error: None,
            })
            .unwrap();

        assert_eq!(
            library
                .metadata_projection_for_asset(&asset)
                .unwrap()
                .unwrap()
                .rating,
            Some(3)
        );
        assert!(
            library
                .metadata_projection_for_asset(&AssetSummary {
                    modified_at_ms: 101,
                    ..asset
                })
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sqlite_order_rejects_an_older_result_from_another_connection() {
        let state = tempdir().unwrap();
        let database = state.path().join("library.sqlite");
        let first = Library::open(&database).unwrap();
        let second = Library::open(&database).unwrap();
        let path = state.path().join("shared.HIF");
        let source_revision = "shared-source";
        let older = first.next_resource_revision().unwrap();
        let newer = second.next_resource_revision().unwrap();

        second
            .accept_metadata_projection(MetadataProjection {
                path: path.clone(),
                source_revision: source_revision.into(),
                state_revision: 0,
                valid_at: newer,
                status: ResourceLoadStatus::Ready,
                rating: Some(5),
                color_label: None,
                pick_label: None,
                error: None,
            })
            .unwrap();
        let winner = first
            .accept_metadata_projection(MetadataProjection {
                path: path.clone(),
                source_revision: source_revision.into(),
                state_revision: 0,
                valid_at: older,
                status: ResourceLoadStatus::Ready,
                rating: Some(1),
                color_label: None,
                pick_label: None,
                error: None,
            })
            .unwrap();

        assert_eq!(winner.valid_at, newer);
        assert_eq!(winner.rating, Some(5));
        assert_eq!(
            first
                .metadata_projection(&path, source_revision)
                .unwrap()
                .unwrap()
                .rating,
            Some(5)
        );
    }

    #[test]
    fn image_projection_round_trips_artifact_without_pixel_payload() {
        let library = Library::in_memory().unwrap();
        let directory = tempdir().unwrap();
        let path = directory.path().join("one.HIF");
        let artifact = directory.path().join("one-preview.jpg");
        std::fs::write(&artifact, b"restart-safe artifact").unwrap();
        let valid_at = library.next_resource_revision().unwrap();
        let accepted = library
            .accept_image_projection(ImageProjection {
                path: path.clone(),
                source_revision: "image-source".into(),
                state_revision: 0,
                valid_at,
                status: ResourceLoadStatus::Ready,
                level: RenderLevel::Preview,
                result: Some(oxy_domain::PreviewResult {
                    geometry: None,
                    path: artifact.clone(),
                    width: 1600,
                    height: 1067,
                    kind: oxy_domain::PreviewKind::Decoded,
                    render_level: RenderLevel::Preview,
                    resource: None,
                    satisfaction: None,
                    persistence: None,
                    diagnostics: None,
                }),
                error: None,
            })
            .unwrap();

        assert!(accepted.state_revision > valid_at);
        let cached = library
            .image_projection(&path, RenderLevel::Preview, "image-source")
            .unwrap()
            .unwrap();
        assert_eq!(cached.result.unwrap().path, artifact);
    }

    #[test]
    fn image_projection_strips_process_local_resource_before_sqlite_persistence() {
        let library = Library::in_memory().unwrap();
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.HIF");
        let artifact = directory.path().join("artifact.jpg");
        std::fs::write(&artifact, b"restart-safe artifact").unwrap();
        let valid_at = library.next_resource_revision().unwrap();
        let projection = ImageProjection {
            path: source.clone(),
            source_revision: "source-revision".into(),
            state_revision: 0,
            valid_at,
            status: ResourceLoadStatus::Ready,
            level: RenderLevel::Preview,
            result: Some(oxy_domain::PreviewResult {
                geometry: None,
                path: artifact,
                width: 16,
                height: 8,
                kind: oxy_domain::PreviewKind::Decoded,
                render_level: RenderLevel::Preview,
                resource: Some(oxy_domain::MediaResourceDescriptor {
                    resource_id: "old-process-resource".into(),
                    url: "oxy-media://localhost/resource/old-process-resource".into(),
                    media_type: "image/jpeg".into(),
                }),
                satisfaction: Some(oxy_domain::MediaSatisfaction::Satisfied),
                persistence: Some(oxy_domain::MediaPersistence::Persisted),
                diagnostics: None,
            }),
            error: None,
        };
        let accepted = library.accept_image_projection(projection).unwrap();
        assert!(accepted.result.unwrap().resource.is_some());

        let restored = library
            .image_projection(&source, RenderLevel::Preview, "source-revision")
            .unwrap()
            .unwrap();
        assert!(restored.result.unwrap().resource.is_none());
    }

    #[test]
    fn pending_resource_only_projection_is_not_restart_persisted() {
        let library = Library::in_memory().unwrap();
        let source = PathBuf::from("pending.HIF");
        let valid_at = library.next_resource_revision().unwrap();
        library
            .accept_image_projection(ImageProjection {
                path: source.clone(),
                source_revision: "source-revision".into(),
                state_revision: 0,
                valid_at,
                status: ResourceLoadStatus::Ready,
                level: RenderLevel::Preview,
                result: Some(oxy_domain::PreviewResult {
                    geometry: None,
                    path: PathBuf::new(),
                    width: 16,
                    height: 8,
                    kind: oxy_domain::PreviewKind::Embedded,
                    render_level: RenderLevel::Preview,
                    resource: Some(oxy_domain::MediaResourceDescriptor {
                        resource_id: "pending-resource".into(),
                        url: "oxy-media://localhost/resource/pending-resource".into(),
                        media_type: "image/jpeg".into(),
                    }),
                    satisfaction: Some(oxy_domain::MediaSatisfaction::Interim),
                    persistence: Some(oxy_domain::MediaPersistence::Pending),
                    diagnostics: None,
                }),
                error: None,
            })
            .unwrap();

        assert!(
            library
                .image_projection(&source, RenderLevel::Preview, "source-revision")
                .unwrap()
                .unwrap()
                .result
                .is_none()
        );
    }

    #[test]
    fn directory_invalidation_blocks_a_late_worker_result() {
        let library = Library::in_memory().unwrap();
        let directory = PathBuf::from("C:/photos");
        let path = directory.join("late.HIF");
        let valid_at = library.next_resource_revision().unwrap();
        let loading = MetadataProjection {
            path: path.clone(),
            source_revision: "source-before-refresh".into(),
            state_revision: 0,
            valid_at,
            status: ResourceLoadStatus::Loading,
            rating: None,
            color_label: None,
            pick_label: None,
            error: None,
        };
        library.accept_metadata_projection(loading.clone()).unwrap();
        library.invalidate_resource_projections(&directory).unwrap();

        let rejected = library
            .accept_metadata_projection(MetadataProjection {
                status: ResourceLoadStatus::Ready,
                rating: Some(5),
                ..loading
            })
            .unwrap();

        assert_eq!(rejected.error.as_deref(), Some("invalidated"));
        assert!(
            library
                .metadata_projection(&path, "source-before-refresh")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sqlite_order_also_rejects_an_older_image_result() {
        let state = tempdir().unwrap();
        let database = state.path().join("library.sqlite");
        let first = Library::open(&database).unwrap();
        let second = Library::open(&database).unwrap();
        let path = state.path().join("shared.HIF");
        let older = first.next_resource_revision().unwrap();
        let newer = second.next_resource_revision().unwrap();
        let old_artifact = state.path().join("old.jpg");
        let new_artifact = state.path().join("new.jpg");
        std::fs::write(&old_artifact, b"old").unwrap();
        std::fs::write(&new_artifact, b"new").unwrap();
        let make_projection = |valid_at, artifact: &Path| ImageProjection {
            path: path.clone(),
            source_revision: "same-image-source".into(),
            state_revision: 0,
            valid_at,
            status: ResourceLoadStatus::Ready,
            level: RenderLevel::Thumbnail,
            result: Some(oxy_domain::PreviewResult {
                geometry: None,
                path: artifact.to_owned(),
                width: 160,
                height: 120,
                kind: oxy_domain::PreviewKind::Embedded,
                render_level: RenderLevel::Thumbnail,
                resource: None,
                satisfaction: None,
                persistence: None,
                diagnostics: None,
            }),
            error: None,
        };

        second
            .accept_image_projection(make_projection(newer, &new_artifact))
            .unwrap();
        let winner = first
            .accept_image_projection(make_projection(older, &old_artifact))
            .unwrap();

        assert_eq!(winner.valid_at, newer);
        assert_eq!(winner.result.unwrap().path, new_artifact);
    }
}
