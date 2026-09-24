//! The audit view of who owns which table.
//!
//! This is deliberately *not* what cache clearing reads, and not where a table
//! is registered. Deletion lives in the module that owns the table
//! ([`crate::cache::index::clear`] and friends), which also lists the tables it
//! creates. This module derives the classification from which namespace a table
//! was declared in and asserts that the two agree:
//!
//! - a table that exists in SQLite but in no namespace fails the tests;
//! - a table declared rebuildable but not emptied by a clear fails too;
//! - a module writing tables owned by the other namespace fails.
//!
//! Nothing may delete rows by table-name pattern. Cache tables are cleared by
//! the module that created them, and [`crate::user`] tables are only ever
//! removed by an explicit, confirmed user action.

/// What a table means to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataClass {
    /// Derived from the photos or from user data. Deleting it costs a rebuild.
    Rebuildable,
    /// Typed, named, confirmed, or assigned by the user. Deleting it costs the
    /// user's work and requires an explicit, confirmed action.
    UserOwned,
}

/// Every table owned by the library, with the class that decides who may
/// delete from it.
///
/// The class is stored nowhere. It follows from which namespace declares the
/// table, so marking a user fact rebuildable means moving the module, not
/// editing a field. This function exists to be audited and asserted against,
/// not to be consulted before deleting.
pub fn tables() -> impl Iterator<Item = (&'static str, DataClass)> {
    crate::cache::tables()
        .map(|table| (table, DataClass::Rebuildable))
        .chain(crate::user::tables().map(|table| (table, DataClass::UserOwned)))
}

/// Tables a cache clear may empty. Nothing outside [`crate::cache`] is here.
pub fn rebuildable_tables() -> impl Iterator<Item = &'static str> {
    crate::cache::tables()
}

/// Tables that survive every cache clear, migration, and re-index.
pub fn user_owned_tables() -> impl Iterator<Item = &'static str> {
    crate::user::tables()
}

/// The class of a known table, if any namespace claims it.
pub fn class_of(table: &str) -> Option<DataClass> {
    tables()
        .find(|(name, _)| *name == table)
        .map(|(_, class)| class)
}

/// Rebuildable tables a cache clear empties of content but never of their
/// allocator or format marker row, as declared by the module that owns them.
///
/// Resetting a revision counter would let a worker that started before the
/// clear publish an older result with a higher revision and win over
/// everything written after. Clearing a schema marker would only force a
/// needless rebuild of an otherwise valid cache.
pub fn preserved_on_clear() -> impl Iterator<Item = &'static str> {
    crate::cache::preserved_on_clear()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;
    use rusqlite::Connection;
    use std::path::Path;

    fn table_names(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table'
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            // FTS5 shadow tables belong to the virtual table they back and are
            // cleared with it; they are not independently owned storage.
            .filter(|name| !name.starts_with("indexed_asset_search_"))
            .collect()
    }

    /// A table that is not in the registry has no defined owner, which means a
    /// future cache clear would not know whether it may delete it.
    #[test]
    fn every_table_has_a_declared_class() {
        let library = Library::in_memory().unwrap();
        let connection = library.connection.lock();
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

    /// The property this split exists to protect: nothing a user typed may
    /// appear in the list a cache clear is allowed to empty.
    #[test]
    fn clearing_the_cache_preserves_every_user_fact() {
        let library = Library::in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        library.add_root(root.path()).unwrap();
        seed_user_facts(&library);

        library.clear_rebuildable_cache().unwrap();

        let connection = library.connection.lock();
        for table in user_owned_tables() {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap_or_else(|error| panic!("{table} must survive a cache clear: {error}"));
            assert!(count > 0, "{table} lost its rows during a cache clear");
        }
        let preserved: Vec<&str> = preserved_on_clear().collect();
        for table in rebuildable_tables() {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
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

    /// Namespace is a promise, not just a directory. A module is allowed to
    /// *read* across the boundary (the index looks up which roots exist) but
    /// never to write: every cross-namespace write goes through a named
    /// function on the owning module, so it shows up in review as a call
    /// instead of hiding inside a SQL string.
    #[test]
    fn no_module_writes_tables_owned_by_the_other_namespace() {
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let offenders = namespace_offenders(&source_root.join("user"), user_owned_tables())
            .into_iter()
            .chain(namespace_offenders(
                &source_root.join("cache"),
                rebuildable_tables(),
            ))
            .collect::<Vec<_>>();
        assert!(
            offenders.is_empty(),
            "modules must not write tables owned by the other namespace:\n{}",
            offenders.join("\n")
        );
    }

    /// Collects `file:line: table` for every SQL literal in `directory` that
    /// writes to a table it does not own.
    fn namespace_offenders(
        directory: &Path,
        owned: impl Iterator<Item = &'static str>,
    ) -> Vec<String> {
        let owned: Vec<&str> = owned.collect();
        let foreign: Vec<&str> = tables()
            .filter(|(name, _)| !owned.contains(name))
            .map(|(name, _)| name)
            .collect();

        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(directory).expect("namespace directory must be readable") {
            let path = entry.expect("directory entry").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("source must be readable");
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            for (index, line) in source.lines().enumerate() {
                let sql = line.trim();
                if !looks_like_sql(sql) {
                    continue;
                }
                for table in &foreign {
                    if writes_table(sql, table) {
                        offenders.push(format!("{name}:{}: {table}", index + 1));
                    }
                }
            }
        }
        offenders
    }

    /// True for lines that carry SQL, whether the statement is inline in an
    /// `execute(...)` call or a standalone string literal. Uppercase verbs only
    /// appear in SQL, so Rust code and prose drop out here.
    fn looks_like_sql(line: &str) -> bool {
        ["DELETE FROM ", "UPDATE ", "INSERT ", "REPLACE "]
            .iter()
            .any(|verb| line.contains(verb))
    }

    fn writes_table(sql: &str, table: &str) -> bool {
        sql.contains(&format!("DELETE FROM {table}"))
            || sql.contains(&format!("INTO {table}"))
            || sql.contains(&format!("UPDATE {table}"))
            || sql.contains(&format!("UPDATE OR REPLACE {table}"))
            || sql.contains(&format!("UPDATE OR IGNORE {table}"))
    }

    fn seed_user_facts(library: &Library) {
        let connection = library.connection.lock();
        connection
            .execute_batch(
                "INSERT INTO custom_tags(id, name, name_key, sort_order) VALUES (1, 'People', 'people', 0);
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
