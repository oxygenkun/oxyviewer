//! Tables declared by the feature that owns their meaning.
crate::storage::table::tables! {
    user create library_roots =
        "path TEXT PRIMARY KEY NOT NULL,
        added_at INTEGER NOT NULL DEFAULT (unixepoch()),
        sort_order INTEGER NOT NULL DEFAULT 0";
    cache create indexed_roots =
        "root_path TEXT PRIMARY KEY NOT NULL,
        indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
        asset_count INTEGER NOT NULL,
        directory_count INTEGER NOT NULL";
    cache create indexed_directory_roots =
        "root_path TEXT PRIMARY KEY NOT NULL,
        indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
        directory_count INTEGER NOT NULL";
    cache create indexed_assets =
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
    cache create indexed_directories =
        "root_path TEXT NOT NULL,
        path TEXT NOT NULL,
        parent_path TEXT NOT NULL,
        name TEXT NOT NULL,
        has_children INTEGER NOT NULL,
        scan_id INTEGER NOT NULL,
        PRIMARY KEY(root_path, path)";
    marker create library_index_sequence = "id INTEGER PRIMARY KEY CHECK(id = 1), next_scan_id INTEGER NOT NULL";
    cache external indexed_asset_search;
    cache create indexed_asset_search_keys =
        "search_rowid INTEGER PRIMARY KEY, root_path TEXT NOT NULL,
         path TEXT NOT NULL, UNIQUE(root_path,path)";
}

pub(crate) fn initialize(connection: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS indexed_asset_search USING fts5(
        path UNINDEXED, root_path UNINDEXED, name, directory, tokenize='unicode61 remove_diacritics 2');
        CREATE INDEX IF NOT EXISTS indexed_assets_parent ON indexed_assets(root_path,parent_path);
        CREATE INDEX IF NOT EXISTS indexed_directories_parent ON indexed_directories(root_path,parent_path);
        INSERT OR IGNORE INTO library_index_sequence(id,next_scan_id) VALUES(1,1);")
}
