//! Register user-owned declarations. Legacy initialization remains until the
//! isolated fresh-schema step removes compatibility paths.

use super::has_column;
use crate::storage::table;
use rusqlite::{Connection, params};

pub static DEFS: std::sync::LazyLock<Vec<table::TableDef>> = std::sync::LazyLock::new(|| {
    super::definitions()
        .iter()
        .copied()
        .filter(|def| def.class == table::DataClass::UserOwned)
        .collect()
});

/// Creates or migrates every user-owned table.
pub(super) fn ensure_schema(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    table::create_all(connection, &DEFS)?;
    connection.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS custom_tags_sibling_name
           ON custom_tags(COALESCE(parent_id, 0), name_key);
         CREATE INDEX IF NOT EXISTS custom_tags_parent
           ON custom_tags(parent_id, sort_order, name);
         CREATE INDEX IF NOT EXISTS asset_tags_tag ON asset_tags(tag_id, asset_path);
         CREATE INDEX IF NOT EXISTS folder_people_folder ON folder_people(folder_path);
         CREATE INDEX IF NOT EXISTS person_manual_instances_asset
           ON person_manual_instances(folder_path, asset_path);
         CREATE INDEX IF NOT EXISTS folder_historical_links_person
           ON folder_historical_links(historical_person_id);",
    )?;
    // Column migrations for files written before the column existed.
    if !has_column(connection, "library_roots", "sort_order")? {
        connection.execute(
            "ALTER TABLE library_roots ADD COLUMN sort_order INTEGER",
            [],
        )?;
    }
    if !has_column(
        connection,
        "person_manual_instances",
        "source_identity_revision",
    )? {
        connection.execute(
            "ALTER TABLE person_manual_instances ADD COLUMN source_identity_revision TEXT",
            [],
        )?;
    }
    normalize_root_order(connection)?;
    ensure_tag_sources(connection)
}

/// Gives every root a dense sort order, so the sidebar keeps the order the user
/// arranged in even after a root is added by an older build that had no column.
fn normalize_root_order(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    let paths = {
        let mut statement = connection.prepare(
            "SELECT path FROM library_roots
             ORDER BY sort_order IS NULL, sort_order, added_at, path",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>("path"))?
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

/// Creates `asset_tag_sources` and derives a `legacy` source for every
/// assignment that predates it.
///
/// This runs in its own transaction because the backfill reads `asset_tags`,
/// which `create_all` above has just created.
fn ensure_tag_sources(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "BEGIN;
         CREATE TABLE IF NOT EXISTS asset_tag_sources (
           asset_path TEXT NOT NULL,
           tag_id INTEGER NOT NULL REFERENCES custom_tags(id) ON DELETE CASCADE,
           source_kind TEXT NOT NULL CHECK(source_kind IN ('legacy','manual','sidecar','person')),
           source_id TEXT NOT NULL DEFAULT '',
           PRIMARY KEY(asset_path,tag_id,source_kind,source_id)
         );
         CREATE INDEX IF NOT EXISTS asset_tag_sources_tag
           ON asset_tag_sources(tag_id,source_kind,asset_path);
         INSERT OR IGNORE INTO asset_tag_sources(asset_path,tag_id,source_kind,source_id)
           SELECT a.asset_path,a.tag_id,'legacy','' FROM asset_tags a
           WHERE NOT EXISTS (SELECT 1 FROM asset_tag_sources s
             WHERE s.asset_path=a.asset_path AND s.tag_id=a.tag_id);
         COMMIT;",
    )
}
