//! Tables declared by the feature that owns their meaning.
crate::storage::table::tables! {
    user create custom_tags =
        "id INTEGER PRIMARY KEY AUTOINCREMENT,
        parent_id INTEGER REFERENCES custom_tags(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        name_key TEXT NOT NULL,
        sort_order INTEGER NOT NULL,
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create asset_tags =
        "asset_path TEXT NOT NULL,
        tag_id INTEGER NOT NULL REFERENCES custom_tags(id) ON DELETE CASCADE,
        assigned_at INTEGER NOT NULL DEFAULT (unixepoch()),
        PRIMARY KEY(asset_path, tag_id)";
    user create asset_tag_xmp_state =
        "asset_path TEXT PRIMARY KEY NOT NULL,
        subjects_json TEXT NOT NULL DEFAULT '[]',
        hierarchical_json TEXT NOT NULL DEFAULT '[]',
        synced_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create tag_xmp_sync_queue =
        "asset_path TEXT PRIMARY KEY NOT NULL,
        requested_at INTEGER NOT NULL DEFAULT (unixepoch()),
        attempt_count INTEGER NOT NULL DEFAULT 0,
        last_error TEXT";
    user create asset_tag_sources =
        "asset_path TEXT NOT NULL, tag_id INTEGER NOT NULL REFERENCES custom_tags(id) ON DELETE CASCADE,
         source_kind TEXT NOT NULL CHECK(source_kind IN ('manual','sidecar','person')),
         source_id TEXT NOT NULL DEFAULT '', PRIMARY KEY(asset_path,tag_id,source_kind,source_id)";
}

pub(crate) fn initialize(connection: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS custom_tags_sibling_name
        ON custom_tags(COALESCE(parent_id,0),name_key);
        CREATE INDEX IF NOT EXISTS custom_tags_parent ON custom_tags(parent_id,sort_order,name);
        CREATE INDEX IF NOT EXISTS asset_tags_tag ON asset_tags(tag_id,asset_path);
        CREATE INDEX IF NOT EXISTS asset_tag_sources_tag ON asset_tag_sources(tag_id,source_kind,asset_path);")
}
