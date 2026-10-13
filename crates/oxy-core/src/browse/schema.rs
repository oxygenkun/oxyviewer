//! Tables declared by the feature that owns their meaning.
crate::storage::table::tables! {
    cache create directory_snapshots =
        "root_path TEXT NOT NULL,
        directory_path TEXT NOT NULL,
        assets_json TEXT NOT NULL,
        PRIMARY KEY(root_path, directory_path)";
}
