//! Explicit library roots: user configuration, not derived state.
//!
//! A root exists only because the user pointed OxyViewer at it. Nothing in
//! this module is rebuilt by a scan, and removing a root is a user decision
//! that never cascades into identifiers, reviews, or tags.

use crate::{Library, LibraryError, cache::index::forget_root};
use rusqlite::{Connection, params};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

// `preserve` because a root is the user's, not something a cache clear may
// empty. Every table in [`crate::user`] is declared this way.
oxy_store::table::tables! {
    preserve library_roots =
        "path TEXT PRIMARY KEY NOT NULL,
        added_at INTEGER NOT NULL DEFAULT (unixepoch()),
        sort_order INTEGER";
}

pub(super) fn ensure_schema(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    oxy_store::table::create_all(connection, DEFS)?;
    let has_sort_order = connection
        .prepare("PRAGMA table_info(library_roots)")?
        .query_map([], |row| row.get::<_, String>("name"))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == "sort_order");
    if !has_sort_order {
        connection.execute(
            "ALTER TABLE library_roots ADD COLUMN sort_order INTEGER",
            [],
        )?;
    }
    normalize_root_order(connection)
}

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

impl Library {
    pub fn add_root(&self, path: &Path) -> Result<(), LibraryError> {
        let canonical = path.canonicalize()?;
        self.write().execute(
            "INSERT OR IGNORE INTO library_roots(path, sort_order)
             VALUES (?1, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM library_roots))",
            params![canonical.to_string_lossy()],
        )?;
        Ok(())
    }

    pub fn remove_root(&self, path: &Path) -> Result<(), LibraryError> {
        // Stored roots are canonical, but removal must also work after a folder
        // has been moved or disconnected. In that case the exact persisted path
        // is still safe to remove.
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.invalidate_snapshot_root(&path)?;
        let root = path.to_string_lossy();
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM library_roots WHERE path = ?1", params![root])?;
        // The index owns its own rows. Asking it to forget the root keeps the
        // delete list next to the schema that defines those tables.
        forget_root(&transaction, root.as_ref())?;
        transaction.commit()?;
        Ok(())
    }

    pub fn roots(&self) -> Result<Vec<PathBuf>, LibraryError> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare("SELECT path FROM library_roots ORDER BY sort_order, added_at, path")?;
        let paths = statement
            .query_map([], |row| row.get::<_, String>("path"))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(paths.into_iter().map(PathBuf::from).collect())
    }

    pub fn reorder_roots(&self, paths: &[PathBuf]) -> Result<(), LibraryError> {
        let mut connection = self.write();
        let stored = connection
            .prepare("SELECT path FROM library_roots")?
            .query_map([], |row| row.get::<_, String>("path"))?
            .collect::<Result<HashSet<_>, _>>()?;
        let requested = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<HashSet<_>>();
        if stored.len() != paths.len() || requested != stored {
            return Err(LibraryError::InvalidRootOrder);
        }

        let transaction = connection.transaction()?;
        for (sort_order, path) in paths.iter().enumerate() {
            transaction.execute(
                "UPDATE library_roots SET sort_order = ?1 WHERE path = ?2",
                params![sort_order as i64, path.to_string_lossy()],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn contains_root(&self, path: &Path) -> Result<bool, LibraryError> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        self.read_connection()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM library_roots WHERE path = ?1) AS present",
                params![path.to_string_lossy()],
                |row| row.get("present"),
            )
            .map_err(Into::into)
    }
}
