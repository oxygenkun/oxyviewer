//! Declarative table definitions.
//!
//! Every table is declared once, as a [`TableDef`], and the three things that
//! used to be written separately are derived from it: the `CREATE TABLE`
//! statement, the list of tables a schema owns, and the `DELETE` statements
//! that empty them. Adding a table is one edit; forgetting to clear it is not
//! possible, because clearing reads the same declaration that created it.
//!
//! This module deliberately knows nothing about *which* tables exist. The
//! schema modules declare them with [`tables!`], and each declaration states
//! its class — derived or user-owned — instead of leaving the class to be
//! inferred from which directory the declaring file happened to sit in. That
//! inference stopped working once one schema file declares tables of every
//! class.
//!
//! This is intentionally not a query builder. The SQL in this application is
//! tuned by hand — FTS5 joins, `source_revision` filters, time-sliced batches,
//! the sqlite-vec distance operator — and an abstraction over it would only
//! hide the parts that matter. What this module borrows from an ORM is the
//! schema being declared in one place, not the queries being generated.

use rusqlite::Connection;

/// What a table means to the user, and therefore who may delete from it.
///
/// The class is a word in the declaration, not a consequence of where the
/// declaration lives, so a reviewer reads it from the table's own line. It is
/// audited by [`crate::schema`] and never consulted before a single row write;
/// only clearing whole caches reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataClass {
    /// Derived from the photos or from user data. Deleting it costs a rebuild,
    /// so a cache clear empties it without asking.
    Rebuildable,
    /// Typed, named, confirmed, or assigned by the user. Deleting it costs the
    /// user's work and requires an explicit, confirmed action.
    UserOwned,
    /// An allocator — a revision counter — or a cache-format marker: neither
    /// derived content nor a user fact. A clear leaves it alone, because
    /// resetting a counter lets a worker that started before the clear publish
    /// an older result and win, and dropping a marker only forces a needless
    /// rebuild of an otherwise valid cache.
    Marker,
}

impl DataClass {
    /// Whether a cache clear empties a table of this class.
    pub fn is_cleared_by_cache_clear(self) -> bool {
        matches!(self, DataClass::Rebuildable)
    }
}

/// One table as declared by the schema module that owns it.
///
/// `created` is separate from the class because some tables are built outside
/// the declaration: an FTS5 virtual table and a table only a backfill
/// transaction creates. Both still participate in a cache clear, so they are
/// declared for that purpose and skipped at creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableDef {
    pub name: &'static str,
    /// Column and constraint definitions, without the surrounding parentheses.
    /// Empty for tables this declaration does not create.
    pub ddl: &'static str,
    /// What the table means, which decides whether a clear may empty it.
    pub class: DataClass,
    /// Whether this declaration creates the table.
    pub created: bool,
}

/// Declares tables by class and creation, in creation order.
///
/// ```ignore
/// tables! {
///     user   create   custom_tags = "id INTEGER PRIMARY KEY, name TEXT NOT NULL";
///     user   external asset_tag_sources;
///     cache  create   indexed_assets = "path TEXT NOT NULL PRIMARY KEY";
///     marker create   library_index_sequence = "id INTEGER PRIMARY KEY CHECK(id = 1)";
///     cache  external indexed_asset_search;
/// }
/// ```
///
/// The first word is the [`DataClass`]: `cache` is emptied by a cache clear,
/// `user` and `marker` are not. The second says whether this declaration
/// creates the table — `create` runs `CREATE TABLE`, `external` records a table
/// that something else builds so it still takes part in a clear.
///
/// The DDL is a string, not a token stream, because it is SQL: `'queued'` is a
/// string literal there and a character literal to the Rust tokenizer.
///
/// Order matters: tables are created in declaration order, so a table must be
/// declared *after* every table it references, and a clear walks the
/// declaration backwards so it always empties the referencing table first.
///
/// `#[macro_export]` places the macro at this crate's root; the `pub use`
/// below re-exports it at `oxy_store::table::tables`, so another crate can
/// invoke it by path next to the type it produces.
#[macro_export]
macro_rules! tables {
    ( $( $class:ident $creation:ident $name:ident $( = $ddl:literal )? );+ $(;)? ) => {
        pub const DEFS: &[$crate::table::TableDef] =
            &[ $( $crate::table::table_def!($class $creation $name $( = $ddl )?) ),+ ];
    };
}

pub use crate::tables;

/// The class-and-creation-to-[`TableDef`] expansion used by [`tables!`].
///
/// Re-exported at `oxy_store::table::table_def` so the `tables!` expansion can
/// reach it by path from a crate that does not import it.
#[macro_export]
macro_rules! table_def {
    (cache create $name:ident = $ddl:literal) => {
        $crate::table::TableDef {
            name: stringify!($name),
            ddl: $ddl,
            class: $crate::table::DataClass::Rebuildable,
            created: true,
        }
    };
    (user create $name:ident = $ddl:literal) => {
        $crate::table::TableDef {
            name: stringify!($name),
            ddl: $ddl,
            class: $crate::table::DataClass::UserOwned,
            created: true,
        }
    };
    (marker create $name:ident = $ddl:literal) => {
        $crate::table::TableDef {
            name: stringify!($name),
            ddl: $ddl,
            class: $crate::table::DataClass::Marker,
            created: true,
        }
    };
    (cache external $name:ident) => {
        $crate::table::TableDef {
            name: stringify!($name),
            ddl: "",
            class: $crate::table::DataClass::Rebuildable,
            created: false,
        }
    };
    (user external $name:ident) => {
        $crate::table::TableDef {
            name: stringify!($name),
            ddl: "",
            class: $crate::table::DataClass::UserOwned,
            created: false,
        }
    };
}

pub use crate::table_def;

/// Creates every declared table that this declaration owns.
pub fn create_all(connection: &Connection, defs: &[TableDef]) -> Result<(), rusqlite::Error> {
    let mut sql = String::new();
    for def in defs.iter().filter(|def| def.created) {
        sql.push_str("CREATE TABLE IF NOT EXISTS ");
        sql.push_str(def.name);
        sql.push_str(" (");
        sql.push_str(def.ddl);
        sql.push_str(");\n");
    }
    connection.execute_batch(&sql)
}

/// Empties every declared table whose class a clear is allowed to empty.
///
/// This walks the declaration backwards. Tables are declared in creation
/// order — a table is created after the table it references — so clearing in
/// reverse always empties the referencing table first and never leaves a
/// foreign key pointing at a row that is about to disappear.
pub fn clear_all(connection: &Connection, defs: &[TableDef]) -> Result<(), rusqlite::Error> {
    let mut sql = String::new();
    for def in defs
        .iter()
        .rev()
        .filter(|def| def.class.is_cleared_by_cache_clear())
    {
        sql.push_str("DELETE FROM ");
        sql.push_str(def.name);
        sql.push_str(";\n");
    }
    connection.execute_batch(&sql)
}

/// Every table a declaration owns, created by it or not.
pub fn names(defs: &'static [TableDef]) -> impl Iterator<Item = &'static str> {
    defs.iter().map(|def| def.name)
}

/// Declared tables a clear leaves in place.
pub fn preserved(defs: &'static [TableDef]) -> impl Iterator<Item = &'static str> {
    defs.iter()
        .filter(|def| !def.class.is_cleared_by_cache_clear())
        .map(|def| def.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    tables! {
        marker create parent_table = "id TEXT PRIMARY KEY NOT NULL, next_value INTEGER NOT NULL";
        cache  create child_table = "id TEXT PRIMARY KEY NOT NULL, parent_id TEXT NOT NULL REFERENCES parent_table(id)";
        cache  external made_elsewhere;
        user   create user_table = "id TEXT PRIMARY KEY NOT NULL";
    }

    #[test]
    fn declares_creation_order_class_and_ownership() {
        let names = names(DEFS).collect::<Vec<_>>();
        assert_eq!(
            names,
            ["parent_table", "child_table", "made_elsewhere", "user_table"]
        );
        assert_eq!(preserved(DEFS).collect::<Vec<_>>(), ["parent_table", "user_table"]);
        assert_eq!(class_of("made_elsewhere"), Some(DataClass::Rebuildable));
        assert_eq!(class_of("user_table"), Some(DataClass::UserOwned));
    }

    fn class_of(name: &str) -> Option<DataClass> {
        DEFS.iter().find(|def| def.name == name).map(|def| def.class)
    }

    #[test]
    fn generates_create_and_clear_sql_from_the_same_declaration() {
        let store = crate::Store::in_memory().unwrap();
        let connection = store.write();
        create_all(&connection, DEFS).unwrap();
        // `made_elsewhere` is declared for clearing only, so it must not exist.
        let created: Vec<String> = connection
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table'
                 AND name NOT LIKE 'sqlite_%' AND name IN ('child_table', 'parent_table',
                 'made_elsewhere', 'user_table') ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get("name"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(created, ["child_table", "parent_table", "user_table"]);
    }

    #[test]
    fn clear_empties_only_what_the_class_allows() {
        let store = crate::Store::in_memory().unwrap();
        let connection = store.write();
        create_all(&connection, DEFS).unwrap();
        // Mirrors how a caller treats `external`: something else creates it,
        // then the declaration still decides whether a clear may empty it.
        connection
            .execute("CREATE TABLE made_elsewhere(id TEXT NOT NULL)", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO parent_table(id, next_value) VALUES ('p', 7)",
                [],
            )
            .unwrap();
        connection
            .execute("INSERT INTO user_table(id) VALUES ('u')", [])
            .unwrap();
        connection
            .execute("INSERT INTO made_elsewhere(id) VALUES ('x')", [])
            .unwrap();
        clear_all(&connection, DEFS).unwrap();
        let external_rows: i64 = connection
            .query_row("SELECT COUNT(*) AS count FROM made_elsewhere", [], |row| {
                row.get("count")
            })
            .unwrap();
        assert_eq!(external_rows, 0, "an external cache table is still cleared");
        let remaining: i64 = connection
            .query_row(
                "SELECT next_value FROM parent_table WHERE id = 'p'",
                [],
                |row| row.get("next_value"),
            )
            .unwrap();
        assert_eq!(remaining, 7, "a marker keeps its row");
        let users: i64 = connection
            .query_row("SELECT COUNT(*) AS count FROM user_table", [], |row| {
                row.get("count")
            })
            .unwrap();
        assert_eq!(users, 1, "a clear never touches a user-owned table");
    }

    mod referenced_parent {
        crate::table::tables! {
            cache create parent_table = "id TEXT PRIMARY KEY NOT NULL";
            cache create child_table = "parent_id TEXT NOT NULL REFERENCES parent_table(id)";
        }
    }

    /// With `PRAGMA foreign_keys = ON`, deleting the parent while the child
    /// still has rows fails. This is the whole reason clearing runs backwards.
    #[test]
    fn clearing_removes_referencing_rows_before_the_rows_they_reference() {
        let store = crate::Store::in_memory().unwrap();
        let connection = store.write();
        create_all(&connection, referenced_parent::DEFS).unwrap();
        connection
            .execute("INSERT INTO parent_table(id) VALUES ('p')", [])
            .unwrap();
        connection
            .execute("INSERT INTO child_table(parent_id) VALUES ('p')", [])
            .unwrap();
        clear_all(&connection, referenced_parent::DEFS).unwrap();
    }
}
