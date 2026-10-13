//! Tables declared by the feature that owns their meaning.
crate::storage::table::tables! {
    marker create resource_projection_sequence = "id INTEGER PRIMARY KEY CHECK(id = 1), next_revision INTEGER NOT NULL";
    cache create resource_projections =
        "path TEXT NOT NULL,
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
        PRIMARY KEY(path, projection_kind)";
}
