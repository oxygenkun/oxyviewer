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
    user external asset_tag_sources;
}
