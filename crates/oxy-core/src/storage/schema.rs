//! Register feature-owned declarations and audit their user/cache/marker classes.
//! Creation and clearing use the same declarations in dependency order; table
//! names are never used to guess deletion ownership.

pub mod cache;
pub mod user;

pub use crate::storage::table::DataClass;

use crate::storage::table;
use rusqlite::Connection;

/// Features are registered once in creation dependency order. Class-specific
/// creation and clearing both derive their views from this same registry.
pub(super) fn definitions() -> &'static [table::TableDef] {
    static DEFS: std::sync::LazyLock<Vec<table::TableDef>> = std::sync::LazyLock::new(|| {
        [
            crate::library::schema::DEFS,
            crate::tags::schema::DEFS,
            crate::people::schema::DEFS,
            crate::browse::schema::DEFS,
            crate::resources::schema::DEFS,
            crate::files::schema::DEFS,
        ]
        .into_iter()
        .flat_map(|defs| defs.iter().copied())
        .collect()
    });
    &DEFS
}

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
    table::names(&cache::DEFS)
}

/// Tables that survive every cache clear, migration, and re-index.
pub fn user_owned_tables() -> impl Iterator<Item = &'static str> {
    table::names(&user::DEFS)
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
    table::preserved(&cache::DEFS)
}

/// The first format of the reorganized core. Future upgrades must explicitly
/// preserve user declarations; unrecognized files are never adopted or cleared.
const APPLICATION_ID: i32 = 0x4f58_5943;
const SCHEMA_VERSION: i32 = 1;

pub(crate) fn validate(connection: &Connection) -> Result<(), crate::storage::StoreError> {
    let application_id: i32 =
        connection.pragma_query_value(None, "application_id", |row| row.get("application_id"))?;
    let version: i32 =
        connection.pragma_query_value(None, "user_version", |row| row.get("user_version"))?;
    let recognized = application_id == APPLICATION_ID && version == SCHEMA_VERSION;
    if !recognized {
        let occupied: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%') AS occupied",
            [], |row| row.get("occupied"))?;
        if application_id != 0 || version != 0 || occupied {
            return Err(crate::storage::StoreError::UnsupportedSchema {
                application_id,
                version,
            });
        }
    }
    Ok(())
}

pub(crate) fn ensure(connection: &mut Connection) -> Result<(), crate::storage::StoreError> {
    validate(connection)?;
    let transaction = connection.transaction()?;
    table::create_all(&transaction, definitions())?;
    crate::library::schema::initialize(&transaction)?;
    crate::tags::schema::initialize(&transaction)?;
    crate::people::schema::initialize(&transaction)?;
    crate::resources::schema::initialize(&transaction)?;
    transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    crate::people::schema::refresh_cache_format(connection)?;
    Ok(())
}

/// Empties every rebuildable table. Never touches a user-owned one.
pub(crate) fn clear_cache(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    cache::clear(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Store;

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
                 INSERT INTO file_operation_recovery(operation_json,phase) VALUES ('{}','prepared');
                 INSERT INTO custom_tags(id, name, name_key, sort_order) VALUES (1, 'People', 'people', 0);
                 INSERT INTO asset_tags(asset_path, tag_id) VALUES ('/photo.jpg', 1);
                 INSERT INTO asset_tag_sources(asset_path, tag_id, source_kind, source_id)
                   VALUES ('/photo.jpg', 1, 'manual', '');
                 INSERT INTO asset_tag_xmp_state(asset_path) VALUES ('/photo.jpg');
                 INSERT INTO tag_xmp_sync_queue(asset_path) VALUES ('/photo.jpg');
                 INSERT INTO folder_people(id, folder_path, display_name, identity_confirmed)
                   VALUES ('person-1', '/folder', 'Ada', 1);
                 INSERT INTO person_cluster_adoptions(folder_path,cluster_id,subject_id) VALUES ('/folder','cluster-1','person-1');
                 INSERT INTO person_manual_instances(id, folder_path, asset_path, source_revision)
                   VALUES ('instance-1', '/folder', '/photo.jpg', '1:1');
                 INSERT INTO global_people(id,display_name) VALUES ('global-1','Ada');
                 INSERT INTO global_person_migrations VALUES ('person-1','global-1');
                 INSERT INTO global_person_reviews VALUES ('instance-1','global-1','belongs',1);
                 INSERT INTO global_person_targets VALUES ('instance-1','global-1');
                 INSERT INTO global_person_references VALUES ('global-1','instance-1');
                 INSERT INTO global_person_events VALUES ('global-request','{}','{}');
                 INSERT INTO global_person_tags VALUES ('global-1',1);
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
