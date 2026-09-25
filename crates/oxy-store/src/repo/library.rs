//! The explicit library roots.
//!
//! A root exists because the user pointed OxyViewer at it. Nothing here is
//! derived, and no cache clear or re-index reaches these rows.

use crate::StoreError;
use rusqlite::{Connection, params};
use std::collections::HashSet;

/// Adds a root after the ones already there, ignoring one that exists.
pub fn insert_root(connection: &Connection, path: &str) -> Result<(), StoreError> {
    connection.execute(
        "INSERT OR IGNORE INTO library_roots(path, sort_order)
         VALUES (?1, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM library_roots))",
        params![path],
    )?;
    Ok(())
}

/// Removes one root. Returns how many rows it removed, so a caller can tell
/// "removed" from "was not there".
pub fn delete_root(connection: &Connection, path: &str) -> Result<usize, StoreError> {
    Ok(connection.execute("DELETE FROM library_roots WHERE path = ?1", params![path])?)
}

/// Every root path in the order the sidebar shows them.
pub fn list_roots(connection: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement =
        connection.prepare("SELECT path FROM library_roots ORDER BY sort_order, added_at, path")?;
    Ok(statement
        .query_map([], |row| row.get::<_, String>("path"))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// The set of stored root paths, for validating a reorder request against the
/// roots that actually exist.
pub fn root_paths(connection: &Connection) -> Result<HashSet<String>, StoreError> {
    let mut statement = connection.prepare("SELECT path FROM library_roots")?;
    Ok(statement
        .query_map([], |row| row.get::<_, String>("path"))?
        .collect::<Result<HashSet<_>, _>>()?)
}

/// Writes one position of a reorder.
pub fn set_sort_order(
    connection: &Connection,
    path: &str,
    sort_order: i64,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE library_roots SET sort_order = ?1 WHERE path = ?2",
        params![sort_order, path],
    )?;
    Ok(())
}

/// Whether this exact path is one of the stored roots.
pub fn contains_root(connection: &Connection, path: &str) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM library_roots WHERE path = ?1) AS present",
        params![path],
        |row| row.get("present"),
    )?)
}
