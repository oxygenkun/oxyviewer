//! Tables that can always be rebuilt from the original photos plus the
//! user-owned facts.
//!
//! Losing any of them costs re-scanning, re-decoding, or a model pass; it never
//! costs a name, a review decision, or a tag the user assigned. Every table
//! here is emptied by [`super::clear_cache`] — except the `marker` ones, which
//! hold an allocator or a cache-format version rather than content.
//!
//! The derived person data lives here, not with the rest of the person tables
//! in [`super::user`]. That is the one place a reader is likely to look for it
//! in the wrong file: a person's identity is user-owned, the face vectors
//! computed from their photographs are not.

use super::has_column;
use crate::table;
use rusqlite::{Connection, OptionalExtension};

/// Bumped when the shape of the detection cache changes. A mismatch drops the
/// cache and rebuilds it instead of migrating rows that are still cheap to
/// regenerate.
const DETECTION_CACHE_SCHEMA_VERSION: i64 = 2;

/// Bumped when the feature-space or vector layout changes. Same treatment as
/// [`DETECTION_CACHE_SCHEMA_VERSION`]: drop and recompute.
const FEATURE_CACHE_SCHEMA_VERSION: i64 = 3;

crate::table::tables! {
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
    // An FTS5 virtual table and the backfill table that gives its rows a stable
    // identity: both are built by `ensure_search_tables`, and both are declared
    // here so a cache clear still empties them.
    cache external indexed_asset_search;
    cache external indexed_asset_search_keys;
    cache create directory_snapshots =
        "root_path TEXT NOT NULL,
        directory_path TEXT NOT NULL,
        assets_json TEXT NOT NULL,
        PRIMARY KEY(root_path, directory_path)";
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
    marker create person_detection_cache_meta = "schema_version INTEGER NOT NULL";
    cache create person_instances_cache =
        "folder_path TEXT NOT NULL,
        asset_path TEXT NOT NULL,
        instance_id TEXT NOT NULL,
        source_revision TEXT NOT NULL,
        producer_fingerprint TEXT NOT NULL,
        pipeline_fingerprint TEXT NOT NULL,
        run_id TEXT NOT NULL,
        face_box TEXT,
        face_landmarks TEXT,
        body_box TEXT,
        face_score REAL,
        body_score REAL,
        association_score REAL,
        updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
        PRIMARY KEY(folder_path,asset_path,producer_fingerprint,instance_id)";
    marker create person_vector_cache_meta = "schema_version INTEGER NOT NULL";
    cache create person_feature_spaces =
        "id TEXT PRIMARY KEY,
        modality TEXT NOT NULL CHECK(modality IN ('face','body')),
        dimension INTEGER NOT NULL CHECK(dimension > 0 AND dimension <= 4096),
        producer_fingerprint TEXT NOT NULL,
        format_version INTEGER NOT NULL DEFAULT 1";
    cache create person_features_cache =
        "feature_row_id INTEGER PRIMARY KEY,
        folder_path TEXT NOT NULL,
        asset_path TEXT NOT NULL,
        instance_id TEXT NOT NULL,
        source_revision TEXT NOT NULL,
        feature_space_id TEXT NOT NULL REFERENCES person_feature_spaces(id),
        pipeline_fingerprint TEXT NOT NULL,
        vector BLOB NOT NULL,
        updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
        UNIQUE(folder_path,asset_path,instance_id,feature_space_id)";
    cache create person_analysis_heads = "folder_path TEXT PRIMARY KEY, generation INTEGER NOT NULL, run_id TEXT NOT NULL";
    cache create person_analysis_runs =
        "run_id TEXT PRIMARY KEY,
        folder_path TEXT NOT NULL,
        pipeline_id TEXT NOT NULL,
        pipeline_fingerprint TEXT NOT NULL,
        generation INTEGER NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('queued','running','completed','failed','cancelled')),
        enumeration_complete INTEGER NOT NULL DEFAULT 0,
        total_tasks INTEGER NOT NULL DEFAULT 0,
        completed_tasks INTEGER NOT NULL DEFAULT 0,
        failed_tasks INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch())";
    cache create person_analysis_requests =
        "request_id TEXT PRIMARY KEY,
        operation TEXT NOT NULL,
        run_id TEXT NOT NULL REFERENCES person_analysis_runs(run_id)";
    cache create person_analysis_tasks =
        "run_id TEXT NOT NULL REFERENCES person_analysis_runs(run_id),
        asset_path TEXT NOT NULL,
        source_revision TEXT NOT NULL,
        stage_id TEXT NOT NULL,
        stage_fingerprint TEXT NOT NULL,
        stage_order INTEGER NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('queued','running','completed','failed')),
        claim_token TEXT,
        error TEXT,
        PRIMARY KEY(run_id,asset_path,stage_id),
        UNIQUE(run_id,asset_path,stage_order)";
}

/// Creates or migrates every rebuildable table.
pub(super) fn ensure_schema(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    table::create_all(connection, DEFS)?;
    connection.execute_batch(
        "INSERT OR IGNORE INTO indexed_directory_roots(root_path, indexed_at, directory_count)
           SELECT root_path, indexed_at, directory_count FROM indexed_roots;
         INSERT OR IGNORE INTO library_index_sequence(id, next_scan_id) VALUES (1, 1);
         INSERT OR IGNORE INTO resource_projection_sequence(id, next_revision) VALUES (1, 1);",
    )?;
    ensure_search_tables(connection)?;
    // Both resets run before the indexes below: a drop takes its indexes with
    // it, so the index step has to be the last thing that touches them.
    reset_detection_cache_if_stale(connection)?;
    reset_feature_cache_if_stale(connection)?;
    migrate_projection_columns(connection)?;
    if !has_column(connection, "person_analysis_tasks", "claim_token")? {
        connection.execute(
            "ALTER TABLE person_analysis_tasks ADD COLUMN claim_token TEXT",
            [],
        )?;
    }
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS indexed_assets_parent
           ON indexed_assets(root_path, parent_path);
         CREATE INDEX IF NOT EXISTS indexed_directories_parent
           ON indexed_directories(root_path, parent_path);
         CREATE INDEX IF NOT EXISTS resource_projections_parent
           ON resource_projections(parent_path);
         CREATE INDEX IF NOT EXISTS person_instances_cache_asset
           ON person_instances_cache(folder_path,asset_path,source_revision,producer_fingerprint);
         CREATE INDEX IF NOT EXISTS person_features_folder_space
           ON person_features_cache(folder_path,feature_space_id,asset_path,instance_id);
         CREATE INDEX IF NOT EXISTS person_analysis_runs_folder
           ON person_analysis_runs(folder_path,generation DESC);
         CREATE INDEX IF NOT EXISTS person_analysis_tasks_next
           ON person_analysis_tasks(run_id,state,asset_path,stage_order);",
    )
}

/// Empties every table declared above whose class allows it.
///
/// The delete list is not written here: it is derived from the same declaration
/// that created the tables, so a new table is cleared by default and declaring
/// it `user` or `marker` is the only way to opt out.
pub(super) fn clear(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    let transaction = connection.transaction()?;
    table::clear_all(&transaction, DEFS)?;
    transaction.commit()
}

/// Builds the full-text search table and the side table that gives its rows a
/// stable identity.
///
/// FTS5 cannot efficiently look up equality predicates on its UNINDEXED path
/// columns, so the rowid of each indexed asset is mirrored into an ordinary
/// table once, and looked up there afterwards.
fn ensure_search_tables(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS indexed_asset_search USING fts5(
           path UNINDEXED,
           root_path UNINDEXED,
           name,
           directory,
           tokenize = 'unicode61 remove_diacritics 2'
         );",
    )?;
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

/// Drops the detection cache when its format marker does not match this build.
fn reset_detection_cache_if_stale(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    let transaction = connection.transaction()?;
    let version: Option<i64> = transaction
        .query_row(
            "SELECT schema_version FROM person_detection_cache_meta LIMIT 1",
            [],
            |row| row.get("schema_version"),
        )
        .optional()?;
    if version != Some(DETECTION_CACHE_SCHEMA_VERSION) {
        transaction.execute_batch(
            "DROP TABLE IF EXISTS person_instances_cache;
             DELETE FROM person_detection_cache_meta;",
        )?;
        transaction.execute(
            "INSERT INTO person_detection_cache_meta(schema_version) VALUES (?1)",
            [DETECTION_CACHE_SCHEMA_VERSION],
        )?;
        // Runs again because the drop above removed a declared table.
        table::create_all(&transaction, DEFS)?;
    }
    transaction.commit()
}

/// Drops the feature cache when its format marker does not match this build.
///
/// Feature spaces go with the vectors they describe: a vector whose dimension
/// or producer no longer matches its space is not a value that can be migrated.
fn reset_feature_cache_if_stale(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    let transaction = connection.transaction()?;
    let current: Option<i64> = transaction
        .query_row(
            "SELECT schema_version FROM person_vector_cache_meta LIMIT 1",
            [],
            |row| row.get("schema_version"),
        )
        .optional()?;
    if current != Some(FEATURE_CACHE_SCHEMA_VERSION) {
        transaction.execute_batch(
            "DROP TABLE IF EXISTS person_features_cache;
             DROP TABLE IF EXISTS person_feature_spaces;
             DELETE FROM person_vector_cache_meta;",
        )?;
        transaction.execute(
            "INSERT INTO person_vector_cache_meta(schema_version) VALUES (?1)",
            [FEATURE_CACHE_SCHEMA_VERSION],
        )?;
        // Runs again because the drops above removed declared tables.
        table::create_all(&transaction, DEFS)?;
    }
    transaction.commit()
}

/// Renames and adds projection columns written by earlier builds.
fn migrate_projection_columns(connection: &Connection) -> Result<(), rusqlite::Error> {
    if !has_column(connection, "resource_projections", "state_revision")?
        && has_column(connection, "resource_projections", "projection_revision")?
    {
        connection.execute(
            "ALTER TABLE resource_projections RENAME COLUMN projection_revision TO state_revision",
            [],
        )?;
    }
    if !has_column(connection, "resource_projections", "pick_label")? {
        connection.execute(
            "ALTER TABLE resource_projections ADD COLUMN pick_label TEXT",
            [],
        )?;
    }
    Ok(())
}
