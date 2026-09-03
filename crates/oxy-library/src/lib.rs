use oxy_domain::{
    AssetKind, AssetQuery, AssetSort, AssetSummary, DirectorySearchMatch, DirectorySummary, Page,
    SortDirection,
};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
};
use thiserror::Error;

const DEFAULT_PAGE_SIZE: usize = 250;
const MAX_PAGE_SIZE: usize = 1_000;

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Filesystem(#[from] oxy_fs::FsError),
}

pub struct Library {
    connection: Mutex<Connection>,
    indexing_roots: Mutex<HashSet<PathBuf>>,
    index_gate: Mutex<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexStats {
    pub asset_count: usize,
    pub directory_count: usize,
}

impl Library {
    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS library_roots (
              path TEXT PRIMARY KEY NOT NULL,
              added_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE TABLE IF NOT EXISTS indexed_roots (
              root_path TEXT PRIMARY KEY NOT NULL,
              indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
              asset_count INTEGER NOT NULL,
              directory_count INTEGER NOT NULL
            );
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
            ",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
            indexing_roots: Mutex::new(HashSet::new()),
            index_gate: Mutex::new(()),
        })
    }

    pub fn in_memory() -> Result<Self, LibraryError> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "
            CREATE TABLE library_roots (
              path TEXT PRIMARY KEY NOT NULL,
              added_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE TABLE indexed_roots (
              root_path TEXT PRIMARY KEY NOT NULL,
              indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
              asset_count INTEGER NOT NULL,
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
            ",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
            indexing_roots: Mutex::new(HashSet::new()),
            index_gate: Mutex::new(()),
        })
    }

    pub fn add_root(&self, path: &Path) -> Result<(), LibraryError> {
        let canonical = path.canonicalize()?;
        self.connection.lock().execute(
            "INSERT OR IGNORE INTO library_roots(path) VALUES (?1)",
            params![canonical.to_string_lossy()],
        )?;
        Ok(())
    }

    pub fn remove_root(&self, path: &Path) -> Result<(), LibraryError> {
        // Stored roots are canonical, but removal must also work after a folder
        // has been moved or disconnected. In that case the exact persisted path
        // is still safe to remove.
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let root = path.to_string_lossy();
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM library_roots WHERE path = ?1", params![root])?;
        transaction.execute(
            "DELETE FROM indexed_roots WHERE root_path = ?1",
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
            "DELETE FROM indexed_asset_search WHERE root_path = ?1",
            params![root],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn roots(&self) -> Result<Vec<PathBuf>, LibraryError> {
        let connection = self.connection.lock();
        let mut statement =
            connection.prepare("SELECT path FROM library_roots ORDER BY added_at, path")?;
        let paths = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(paths.into_iter().map(PathBuf::from).collect())
    }

    pub fn contains_root(&self, path: &Path) -> Result<bool, LibraryError> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        self.connection
            .lock()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM library_roots WHERE path = ?1)",
                params![path.to_string_lossy()],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    /// Rebuilds one root in low-cost directory batches. Existing completed
    /// rows remain queryable until the final cleanup transaction, and only one
    /// worker per canonical root is admitted at a time.
    pub fn index_root(&self, root: &Path) -> Result<Option<IndexStats>, LibraryError> {
        let root = root.canonicalize()?;
        {
            let mut active = self.indexing_roots.lock();
            if !active.insert(root.clone()) {
                return Ok(None);
            }
        }
        let _gate = self.index_gate.lock();
        let result = self.index_root_inner(&root);
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
        if query.minimum_rating.is_some() || query.color_label.is_some() {
            return Ok(None);
        }
        let root = root.to_string_lossy();
        let connection = self.connection.lock();
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
        let connection = self.connection.lock();
        if !has_completed_index(&connection, &root)? {
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
        let connection = self.connection.lock();
        if !has_completed_index(&connection, &root_text)? {
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
        self.connection.lock().execute(
            "DELETE FROM indexed_roots WHERE root_path = ?1",
            params![root.to_string_lossy()],
        )?;
        Ok(())
    }

    fn index_root_inner(&self, root: &Path) -> Result<IndexStats, LibraryError> {
        let scan_id = self.next_scan_id()?;
        let mut queue = VecDeque::from([root.to_owned()]);
        let mut visited = HashSet::from([root.to_owned()]);
        let mut asset_count = 0;
        let mut directory_count = 0;

        while let Some(directory) = queue.pop_front() {
            if !self.contains_root(root)? {
                return Ok(IndexStats {
                    asset_count,
                    directory_count,
                });
            }
            // Preserve the last completed generation if any directory becomes
            // unreadable. Publishing a partial scan as complete would turn a
            // transient permission or volume error into false deletions.
            let scan = oxy_fs::scan_index_directory(&directory)?;
            let mut safe_directories = Vec::new();
            for mut child in scan.directories {
                let Ok(canonical) = child.path.canonicalize() else {
                    continue;
                };
                if !canonical.starts_with(root) || !visited.insert(canonical.clone()) {
                    continue;
                }
                child.path = canonical.clone();
                queue.push_back(canonical);
                safe_directories.push(child);
            }
            asset_count += scan.assets.len();
            directory_count += safe_directories.len();
            self.write_index_batch(root, &directory, scan_id, &scan.assets, &safe_directories)?;
        }
        self.finish_index(root, scan_id, asset_count, directory_count)?;
        Ok(IndexStats {
            asset_count,
            directory_count,
        })
    }

    fn write_index_batch(
        &self,
        root: &Path,
        parent: &Path,
        scan_id: i64,
        assets: &[AssetSummary],
        directories: &[DirectorySummary],
    ) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        if !root_is_registered(&transaction, &root.to_string_lossy())? {
            return Ok(());
        }
        for asset in assets {
            transaction.execute(
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
                params![
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
                ],
            )?;
        }
        for directory in directories {
            transaction.execute(
                "INSERT INTO indexed_directories(
                   root_path, path, parent_path, name, has_children, scan_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(root_path, path) DO UPDATE SET
                   root_path=excluded.root_path, parent_path=excluded.parent_path,
                   name=excluded.name, has_children=excluded.has_children,
                   scan_id=excluded.scan_id",
                params![
                    root.to_string_lossy(),
                    directory.path.to_string_lossy(),
                    parent.to_string_lossy(),
                    directory.name,
                    directory.has_children,
                    scan_id,
                ],
            )?;
        }
        transaction.commit()?;
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
            "DELETE FROM indexed_assets WHERE root_path = ?1 AND scan_id != ?2",
            params![root.to_string_lossy(), scan_id],
        )?;
        transaction.execute(
            "DELETE FROM indexed_directories WHERE root_path = ?1 AND scan_id != ?2",
            params![root.to_string_lossy(), scan_id],
        )?;
        transaction.execute(
            "DELETE FROM indexed_asset_search WHERE root_path = ?1",
            params![root.to_string_lossy()],
        )?;
        transaction.execute(
            "INSERT INTO indexed_asset_search(path, root_path, name, directory)
             SELECT path, root_path, name, parent_path FROM indexed_assets
             WHERE root_path = ?1",
            params![root.to_string_lossy()],
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

        let directory_matches = library
            .search_directories(&canonical_root, "coa")
            .unwrap()
            .unwrap();
        assert_eq!(directory_matches.len(), 1);
        assert_eq!(directory_matches[0].directory.name, "Coast");
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
}
