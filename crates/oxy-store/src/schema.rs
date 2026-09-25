//! Table classification and the schema step.
//!
//! This crate owns every table: the declaration, the `CREATE TABLE`, the
//! migration that upgrades an older file, and the `DELETE` that empties a
//! derived table. [`table`] holds the mechanism; this module holds the
//! declarations, split by class into [`cache`] and [`user`].
//!
//! The class is a word in each declaration rather than a consequence of the
//! directory the declaring file sits in, because after this change one schema
//! file declares tables of every class — a person's cached face vectors are
//! derived, the person's name is not, and they are declared a few lines apart.
//!
//! Two invariants are asserted here rather than trusted:
//!
//! - every table that exists in the database is declared, so a future cache
//!   clear knows whether it may delete its rows;
//! - emptying every rebuildable table leaves every user-owned table with its
//!   rows, which is the whole reason the two classes are separated.

pub mod cache;
pub mod user;

pub use crate::table::DataClass;

use crate::table;
use rusqlite::Connection;

/// Every table the schema declares, with the class that decides who may
/// delete from it.
///
/// This exists to be audited and asserted against, not to be consulted before
/// deleting: a clear reads the declaration's `DataClass` directly.
pub fn tables() -> impl Iterator<Item = (&'static str, DataClass)> {
    cache::DEFS
        .iter()
        .chain(user::DEFS.iter())
        .map(|def| (def.name, def.class))
}

/// Tables a cache clear may empty.
pub fn rebuildable_tables() -> impl Iterator<Item = &'static str> {
    table::names(cache::DEFS)
}

/// Tables that survive every cache clear, migration, and re-index.
pub fn user_owned_tables() -> impl Iterator<Item = &'static str> {
    table::names(user::DEFS)
}

/// The class of a declared table, if the schema claims it.
pub fn class_of(name: &str) -> Option<DataClass> {
    tables()
        .find(|(table, _)| *table == name)
        .map(|(_, class)| class)
}

/// Rebuildable tables a cache clear empties of content but never of their
/// allocator or format marker row.
///
/// Resetting a revision counter would let a worker that started before the
/// clear publish an older result with a higher revision and win over
/// everything written after. Clearing a schema marker would only force a
/// needless rebuild of an otherwise valid cache.
pub fn preserved_on_clear() -> impl Iterator<Item = &'static str> {
    table::preserved(cache::DEFS)
}

/// Creates or migrates every declared table.
///
/// User-owned tables come first. A cache migration must never be the first
/// thing to define user storage, and the order is now readable in one place
/// instead of inferred from directory layout.
pub(crate) fn ensure(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    user::ensure_schema(connection)?;
    cache::ensure_schema(connection)
}

/// Empties every rebuildable table. Never touches a user-owned one.
pub(crate) fn clear_cache(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    cache::clear(connection)
}

/// Whether `table` has `column`, for the migrations that add one.
///
/// A missing table answers `false`, so the same call covers "column absent"
/// and "table not created yet".
pub(crate) fn has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, rusqlite::Error> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2) AS present",
        rusqlite::params![table, column],
        |row| row.get("present"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn table_names(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table'
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>("name"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            // FTS5 shadow tables belong to the virtual table they back and are
            // cleared with it; they are not independently owned storage.
            .filter(|name| !name.starts_with("indexed_asset_search_"))
            .collect()
    }

    /// A table that is not declared has no defined owner, so a future cache
    /// clear would not know whether it may delete its rows.
    #[test]
    fn every_table_has_a_declared_class() {
        let store = Store::in_memory().unwrap();
        let connection = store.write();
        let undeclared: Vec<String> = table_names(&connection)
            .into_iter()
            .filter(|name| class_of(name).is_none())
            .collect();
        assert!(undeclared.is_empty(), "undeclared tables: {undeclared:?}");
    }

    #[test]
    fn no_table_is_declared_twice() {
        let mut seen = std::collections::HashSet::new();
        for (table, _) in tables() {
            assert!(seen.insert(table), "duplicate table entry: {table}");
        }
    }

    /// The property the two classes exist to protect: nothing a user typed may
    /// appear in the list a cache clear is allowed to empty.
    #[test]
    fn clearing_the_cache_preserves_every_user_fact() {
        let store = Store::in_memory().unwrap();
        seed_user_facts(&store);

        store.clear_rebuildable_cache().unwrap();

        let connection = store.write();
        for table in user_owned_tables() {
            let count: i64 = connection
                .query_row(
                    &format!("SELECT COUNT(*) AS count FROM {table}"),
                    [],
                    |row| row.get("count"),
                )
                .unwrap_or_else(|error| panic!("{table} must survive a cache clear: {error}"));
            assert!(count > 0, "{table} lost its rows during a cache clear");
        }
        let preserved: Vec<&str> = preserved_on_clear().collect();
        for table in rebuildable_tables() {
            let count: i64 = connection
                .query_row(
                    &format!("SELECT COUNT(*) AS count FROM {table}"),
                    [],
                    |row| row.get("count"),
                )
                .unwrap_or_else(|error| panic!("{table} must be queryable: {error}"));
            let expected = i64::from(preserved.contains(&table));
            assert_eq!(
                count,
                expected,
                "{table} was {} during a cache clear",
                if expected == 1 {
                    "emptied"
                } else {
                    "not cleared"
                }
            );
        }
    }

    fn seed_user_facts(store: &Store) {
        store
            .write()
            .execute_batch(
                "INSERT INTO library_roots(path, sort_order) VALUES ('/photos', 0);
                 INSERT INTO custom_tags(id, name, name_key, sort_order) VALUES (1, 'People', 'people', 0);
                 INSERT INTO asset_tags(asset_path, tag_id) VALUES ('/photo.jpg', 1);
                 INSERT INTO asset_tag_sources(asset_path, tag_id, source_kind, source_id)
                   VALUES ('/photo.jpg', 1, 'manual', '');
                 INSERT INTO asset_tag_xmp_state(asset_path) VALUES ('/photo.jpg');
                 INSERT INTO tag_xmp_sync_queue(asset_path) VALUES ('/photo.jpg');
                 INSERT INTO folder_people(id, folder_path, display_name, identity_confirmed)
                   VALUES ('person-1', '/folder', 'Ada', 1);
                 INSERT INTO person_manual_instances(id, folder_path, asset_path, source_revision)
                   VALUES ('instance-1', '/folder', '/photo.jpg', '1:1');
                 INSERT INTO person_review_decisions(instance_id, subject_id, decision)
                   VALUES ('instance-1', 'person-1', 'belongs');
                 INSERT INTO person_review_events(instance_id, subject_id, decision, revision, request_id)
                   VALUES ('instance-1', 'person-1', 'belongs', 1, 'request-1');
                 INSERT INTO person_identity_events(subject_id, event_kind, display_name, revision, request_id)
                   VALUES ('person-1', 'rename', 'Ada', 1, 'request-2');
                 INSERT INTO person_instance_events(instance_id, previous_json, request_id)
                   VALUES ('instance-1', '{}', 'request-instance');
                 INSERT INTO person_references(subject_id, instance_id, source_revision)
                   VALUES ('person-1', 'instance-1', '1:1');
                 INSERT INTO historical_people(id, display_name, reference_asset_path,
                     reference_source_revision)
                   VALUES ('history-1', 'Ada', '/photo.jpg', '1:1');
                 INSERT INTO folder_historical_links(subject_id, historical_person_id)
                   VALUES ('person-1', 'history-1');
                 INSERT INTO person_history_events(subject_id, historical_person_id, event_kind,
                     request_id)
                   VALUES ('person-1', 'history-1', 'link', 'request-4');
                 INSERT INTO person_tag_links(historical_person_id, tag_id, enabled, revision)
                   VALUES ('history-1', 1, 1, 1);
                 INSERT INTO person_tag_overrides(historical_person_id, asset_path, suppressed,
                     revision)
                   VALUES ('history-1', '/photo.jpg', 1, 1);
                 INSERT INTO person_request_results(request_id, operation, entity_id)
                   VALUES ('request-1', 'setPersonReview', 'person-1');",
            )
            .expect("user facts must seed");
    }
}
