//! Register derived and marker declarations. Only derived classes are cleared;
//! format invalidation and column compatibility are still being migrated.

use super::has_column;
use crate::storage::table;
use rusqlite::{Connection, OptionalExtension};

/// Bumped when the shape of the detection cache changes. A mismatch drops the
/// cache and rebuilds it instead of migrating rows that are still cheap to
/// regenerate.
const DETECTION_CACHE_SCHEMA_VERSION: i64 = 2;

/// Bumped when the feature-space or vector layout changes. Same treatment as
/// [`DETECTION_CACHE_SCHEMA_VERSION`]: drop and recompute.
const FEATURE_CACHE_SCHEMA_VERSION: i64 = 3;

pub static DEFS: std::sync::LazyLock<Vec<table::TableDef>> = std::sync::LazyLock::new(|| {
    super::definitions()
        .iter()
        .copied()
        .filter(|def| def.class != table::DataClass::UserOwned)
        .collect()
});

/// Creates or migrates every rebuildable table.
pub(super) fn ensure_schema(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    table::create_all(connection, &DEFS)?;
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
    table::clear_all(&transaction, &DEFS)?;
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
        table::create_all(&transaction, &DEFS)?;
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
        table::create_all(&transaction, &DEFS)?;
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
