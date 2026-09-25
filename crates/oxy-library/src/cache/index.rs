//! Rebuildable asset and directory index.
//!
//! Derived from the volumes behind the explicit roots in [`crate::user::roots`].
//! Deleting every row here only costs a re-scan; no user-owned fact is stored
//! in these tables.

use crate::{
    DEFAULT_PAGE_SIZE, INDEX_WRITE_BATCH_SIZE, INDEX_WRITE_TIME_SLICE, Library, LibraryError,
    MAX_PAGE_SIZE,
};
use oxy_domain::{
    AssetKind, AssetQuery, AssetSort, AssetSummary, DirectorySearchMatch, DirectorySummary, Page,
    SortDirection,
};
use parking_lot::MutexGuard;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

oxy_store::table::tables! {
    clear indexed_roots =
        "root_path TEXT PRIMARY KEY NOT NULL,
        indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
        asset_count INTEGER NOT NULL,
        directory_count INTEGER NOT NULL";
    clear indexed_directory_roots =
        "root_path TEXT PRIMARY KEY NOT NULL,
        indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
        directory_count INTEGER NOT NULL";
    preserve library_index_sequence = "id INTEGER PRIMARY KEY CHECK(id = 1), next_scan_id INTEGER NOT NULL";
    clear indexed_assets =
        "root_path TEXT NOT NULL,
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
        PRIMARY KEY(root_path, path)";
    clear indexed_directories =
        "root_path TEXT NOT NULL,
        path TEXT NOT NULL,
        parent_path TEXT NOT NULL,
        name TEXT NOT NULL,
        has_children INTEGER NOT NULL,
        scan_id INTEGER NOT NULL,
        PRIMARY KEY(root_path, path)";
    // A virtual table and a backfill table: created below, cleared here.
    external indexed_asset_search;
    external indexed_asset_search_keys;
}

pub(super) fn ensure_schema(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    oxy_store::table::create_all(connection, DEFS)?;
    connection.execute_batch(
        "INSERT OR IGNORE INTO indexed_directory_roots(root_path, indexed_at, directory_count)
           SELECT root_path, indexed_at, directory_count FROM indexed_roots;
         INSERT OR IGNORE INTO library_index_sequence(id, next_scan_id) VALUES (1, 1);
         CREATE INDEX IF NOT EXISTS indexed_assets_parent
           ON indexed_assets(root_path, parent_path);
         CREATE INDEX IF NOT EXISTS indexed_directories_parent
           ON indexed_directories(root_path, parent_path);
         CREATE VIRTUAL TABLE IF NOT EXISTS indexed_asset_search USING fts5(
           path UNINDEXED,
           root_path UNINDEXED,
           name,
           directory,
           tokenize = 'unicode61 remove_diacritics 2'
         );
         ",
    )?;
    ensure_search_keys(connection)
}

/// Empties every table declared above.
///
/// The delete list is not written here: it is derived from the same
/// declaration that created the tables, so a new table is cleared by default
/// and marking one `preserve` is the only way to opt out.
pub(super) fn clear(connection: &Connection) -> Result<(), rusqlite::Error> {
    oxy_store::table::clear_all(connection, DEFS)
}

/// Drops every derived row for one root.
///
/// Removing a root is a user decision, but only the cache namespace may delete
/// cache rows. Callers in [`crate::user`] go through this function so a
/// cross-namespace delete shows up as a named call instead of raw SQL.
pub(crate) fn forget_root(
    transaction: &Transaction<'_>,
    root: &str,
) -> Result<(), rusqlite::Error> {
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
    Ok(())
}

/// Give FTS records an indexed, stable identity. FTS5 cannot efficiently look up
/// equality predicates on its UNINDEXED path columns. Preserve existing rowids
/// during migration so an interrupted index can resume without rebuilding search.
fn ensure_search_keys(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    let transaction = connection.transaction()?;
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table'
         AND name = 'indexed_asset_search_keys') AS present",
        [],
        |row| row.get("present"),
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

pub(crate) struct DirectoryPriorityQueue {
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
            "SELECT EXISTS(SELECT 1 FROM library_roots WHERE path = ?1) AS registered",
            params![root],
            |row| row.get::<_, bool>("registered"),
        )?;
        Ok(registered && !has_completed_index(&connection, &root)?)
    }

    pub fn index_root_with_progress(
        &self,
        root: &Path,
        mut report_progress: impl FnMut(IndexProgress),
    ) -> Result<Option<IndexStats>, LibraryError> {
        self.index_root_with_progress_and_directory_scan(root, &mut report_progress, |directory| {
            oxy_fs::scan_index_directories_with_checkpoint(directory, || {
                self.foreground.wait_for_background();
            })
        })
    }

    /// Rebuilds an index while allowing the application-owned filesystem
    /// catalog to share immediate-child directory scans with its tree worker.
    pub fn index_root_with_progress_and_directory_scan(
        &self,
        root: &Path,
        mut report_progress: impl FnMut(IndexProgress),
        mut scan_directories: impl FnMut(&Path) -> Result<Vec<DirectorySummary>, oxy_fs::FsError>,
    ) -> Result<Option<IndexStats>, LibraryError> {
        let root = root.canonicalize()?;
        {
            let mut active = self.indexing_roots.lock();
            if !active.insert(root.clone()) {
                return Ok(None);
            }
        }
        let _gate = self.index_gate.lock();
        let result = self.index_root_inner(&root, &mut report_progress, &mut scan_directories);
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
        // The index stores ratings, colors, and flags but the paged query path
        // here only serves cheap filters; metadata filters fall back to a
        // scanned-and-enriched page.
        if query.needs_metadata_enrichment() || !query.tag_ids.is_empty() {
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
                "SELECT COUNT(*) AS total
                 FROM indexed_assets a
                 JOIN indexed_asset_search s ON s.path = a.path AND s.root_path = a.root_path
                 WHERE s.root_path = ?1
                   AND indexed_asset_search MATCH ?2
                   AND (?3 IS NULL OR a.kind = ?3)",
                params![root, expression, kind],
                |row| row.get::<_, usize>("total"),
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
                "SELECT COUNT(*) AS total FROM indexed_assets a
                 WHERE a.root_path = ?1 AND a.parent_path = ?2
                   AND (?3 IS NULL OR a.kind = ?3)",
                params![root, directory, kind],
                |row| row.get::<_, usize>("total"),
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
                    path: PathBuf::from(row.get::<_, String>("path")?),
                    name: row.get("name")?,
                    has_children: row.get("has_children")?,
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
                    path: PathBuf::from(row.get::<_, String>("path")?),
                    name: row.get("name")?,
                    has_children: row.get("has_children")?,
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
        let mut connection = self.write();
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
        scan_directories: &mut impl FnMut(&Path) -> Result<Vec<DirectorySummary>, oxy_fs::FsError>,
    ) -> Result<IndexStats, LibraryError> {
        let index_started = Instant::now();
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
            let directories = scan_directories(&directory)?;
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
            let assets = if let Some(assets) =
                self.validated_directory_assets(root, directory, index_started)
            {
                assets
            } else {
                Arc::new(oxy_fs::scan_assets_with_progress(directory, |_| {
                    self.foreground.wait_for_background();
                })?)
            };
            asset_count += assets.len();
            self.write_asset_index_batch(root, directory, scan_id, assets.as_ref())?;
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
            let mut connection = self.write();
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
            let mut connection = self.write();
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

    pub(crate) fn write_asset_index_batch(
        &self,
        root: &Path,
        parent: &Path,
        scan_id: i64,
        assets: &[AssetSummary],
    ) -> Result<(), LibraryError> {
        let mut offset = 0;
        while offset < assets.len() {
            let batch = &assets[offset..assets.len().min(offset + INDEX_WRITE_BATCH_SIZE)];
            let mut connection = self.write();
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
                        |row| row.get("search_rowid"),
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

    pub(crate) fn next_scan_id(&self) -> Result<i64, LibraryError> {
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let scan_id = transaction.query_row(
            "SELECT next_scan_id FROM library_index_sequence WHERE id = 1",
            [],
            |row| row.get("next_scan_id"),
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
        let mut connection = self.write();
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

    pub(crate) fn finish_index(
        &self,
        root: &Path,
        scan_id: i64,
        asset_count: usize,
        directory_count: usize,
    ) -> Result<(), LibraryError> {
        let mut connection = self.write();
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

pub(crate) fn has_completed_index(
    connection: &Connection,
    root: &str,
) -> Result<bool, rusqlite::Error> {
    connection
        .query_row(
            "SELECT 1 FROM indexed_roots WHERE root_path = ?1",
            params![root],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
}

pub(crate) fn has_completed_directory_index(
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

pub(crate) fn fts_expression(search: &str) -> String {
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

pub(crate) fn asset_from_row(row: &rusqlite::Row<'_>) -> Result<AssetSummary, rusqlite::Error> {
    Ok(AssetSummary {
        id: row.get("id")?,
        path: PathBuf::from(row.get::<_, String>("path")?),
        name: row.get("name")?,
        extension: row.get("extension")?,
        kind: parse_kind(&row.get::<_, String>("kind")?)?,
        size_bytes: row.get("size_bytes")?,
        modified_at_ms: row.get("modified_at_ms")?,
        has_sidecar: row.get("has_sidecar")?,
        rating: None,
        color_label: None,
        pick_label: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
}
