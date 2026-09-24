//! Declarative table definitions.
//!
//! Every table is declared once, as a [`TableDef`], and the three things that
//! used to be written separately are derived from it: the `CREATE TABLE`
//! statement, the list of tables a module owns, and the `DELETE` statements
//! that empty them. Adding a table is one edit; forgetting to clear it is not
//! possible, because clearing reads the same declaration that created it.
//!
//! This is intentionally not a query builder. The SQL in this crate is tuned by
//! hand — FTS5 joins, `source_revision` filters, time-sliced batches, the
//! sqlite-vec distance operator — and an abstraction over it would only hide
//! the parts that matter. What this module borrows from an ORM is the schema
//! being declared in one place, not the queries being generated.

use rusqlite::Connection;

/// One table as declared by the module that owns it.
///
/// `cleared` and `created` are separate because two tables in this crate are
/// created outside the declaration: an FTS5 virtual table and a migration table
/// that only exists after its backfill has run. Both still participate in a
/// cache clear, so they are declared for that purpose and skipped at creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableDef {
    pub name: &'static str,
    /// Column and constraint definitions, without the surrounding parentheses.
    /// Empty for tables this declaration does not create.
    pub ddl: &'static str,
    /// Whether a cache clear empties this table.
    pub cleared: bool,
    /// Whether this declaration creates the table.
    pub created: bool,
}

/// Declares the tables owned by a module.
///
/// ```ignore
/// tables! {
///     clear indexed_assets =
///         "root_path TEXT NOT NULL,
///          path TEXT NOT NULL,
///          PRIMARY KEY(root_path, path)";
///     preserve library_index_sequence =
///         "id INTEGER PRIMARY KEY CHECK(id = 1),
///          next_scan_id INTEGER NOT NULL";
///     external indexed_asset_search;
/// }
/// ```
///
/// The DDL is a string, not a token stream, because it is SQL: `'queued'` is a
/// string literal there and a character literal to the Rust tokenizer.
///
/// `clear` creates the table and empties it on a cache clear. `preserve`
/// creates it but leaves its rows alone, which is what revision counters and
/// cache format markers need. `external` declares a table created elsewhere so
/// it still takes part in a clear.
///
/// Order matters: tables are created in declaration order and cleared in the
/// same order, so a table that references another must be declared before it.
macro_rules! tables {
    ( $( $mode:ident $name:ident $( = $ddl:literal )? );+ $(;)? ) => {
        pub(crate) const DEFS: &[crate::table::TableDef] =
            &[ $( $crate::table::table_def!($mode $name $( = $ddl )?) ),+ ];
    };
}

pub(crate) use tables;

macro_rules! table_def {
    (clear $name:ident = $ddl:literal) => {
        crate::table::TableDef {
            name: stringify!($name),
            ddl: $ddl,
            cleared: true,
            created: true,
        }
    };
    (preserve $name:ident = $ddl:literal) => {
        crate::table::TableDef {
            name: stringify!($name),
            ddl: $ddl,
            cleared: false,
            created: true,
        }
    };
    (external $name:ident) => {
        crate::table::TableDef {
            name: stringify!($name),
            ddl: "",
            cleared: true,
            created: false,
        }
    };
}

pub(crate) use table_def;

/// Creates every declared table that this declaration owns.
pub(crate) fn create_all(
    connection: &Connection,
    defs: &[TableDef],
) -> Result<(), rusqlite::Error> {
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

/// Empties every declared table that a clear is allowed to empty.
///
/// This walks the declaration backwards. Tables are declared in creation
/// order — a table is created after the table it references — so clearing in
/// reverse always empties the referencing table first and never leaves a
/// foreign key pointing at a row that is about to disappear.
pub(crate) fn clear_all(connection: &Connection, defs: &[TableDef]) -> Result<(), rusqlite::Error> {
    let mut sql = String::new();
    for def in defs.iter().rev().filter(|def| def.cleared) {
        sql.push_str("DELETE FROM ");
        sql.push_str(def.name);
        sql.push_str(";\n");
    }
    connection.execute_batch(&sql)
}

/// Every table a module declares, created by it or not.
pub(crate) fn names(defs: &'static [TableDef]) -> impl Iterator<Item = &'static str> {
    defs.iter().map(|def| def.name)
}

/// Declared tables a clear leaves in place.
pub(crate) fn preserved(defs: &'static [TableDef]) -> impl Iterator<Item = &'static str> {
    defs.iter().filter(|def| !def.cleared).map(|def| def.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    tables! {
        preserve parent_table = "id TEXT PRIMARY KEY NOT NULL, next_value INTEGER NOT NULL";
        clear child_table = "id TEXT PRIMARY KEY NOT NULL, parent_id TEXT NOT NULL REFERENCES parent_table(id)";
        external made_elsewhere;
    }

    #[test]
    fn declares_creation_order_clear_order_and_ownership() {
        let names = names(DEFS).collect::<Vec<_>>();
        assert_eq!(names, ["parent_table", "child_table", "made_elsewhere"]);
        assert_eq!(preserved(DEFS).collect::<Vec<_>>(), ["parent_table"]);
    }

    #[test]
    fn generates_create_and_clear_sql_from_the_same_declaration() {
        let library = crate::Library::in_memory().unwrap();
        let connection = library.connection.lock();
        create_all(&connection, DEFS).unwrap();
        // `made_elsewhere` is declared for clearing only, so it must not exist.
        let created: Vec<String> = connection
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table'
                 AND name NOT LIKE 'sqlite_%' AND name IN ('child_table', 'parent_table',
                 'made_elsewhere') ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get("name"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(created, ["child_table", "parent_table"]);
    }

    #[test]
    fn clear_empties_only_what_the_declaration_allows() {
        let library = crate::Library::in_memory().unwrap();
        let connection = library.connection.lock();
        create_all(&connection, DEFS).unwrap();
        // Mirrors how the crate treats `external`: something else creates it,
        // then the declaration still decides it is cleared.
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
            .execute("INSERT INTO made_elsewhere(id) VALUES ('x')", [])
            .unwrap();
        clear_all(&connection, DEFS).unwrap();
        let external_rows: i64 = connection
            .query_row("SELECT COUNT(*) AS count FROM made_elsewhere", [], |row| {
                row.get("count")
            })
            .unwrap();
        assert_eq!(external_rows, 0, "an external table is still cleared");
        let remaining: i64 = connection
            .query_row(
                "SELECT next_value FROM parent_table WHERE id = 'p'",
                [],
                |row| row.get("next_value"),
            )
            .unwrap();
        assert_eq!(remaining, 7, "a preserved table keeps its rows");
    }

    mod referenced_parent {
        crate::table::tables! {
            clear parent_table = "id TEXT PRIMARY KEY NOT NULL";
            clear child_table = "parent_id TEXT NOT NULL REFERENCES parent_table(id)";
        }
    }

    /// With `PRAGMA foreign_keys = ON`, deleting the parent while the child
    /// still has rows fails. This is the whole reason clearing runs backwards.
    #[test]
    fn clearing_removes_referencing_rows_before_the_rows_they_reference() {
        let library = crate::Library::in_memory().unwrap();
        let connection = library.connection.lock();
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
