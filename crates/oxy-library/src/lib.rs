use parking_lot::Mutex;
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct Library {
    connection: Mutex<Connection>,
}

impl Library {
    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS library_roots (
              path TEXT PRIMARY KEY NOT NULL,
              added_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE TABLE IF NOT EXISTS assets (
              path TEXT PRIMARY KEY NOT NULL,
              name TEXT NOT NULL,
              kind TEXT NOT NULL,
              modified_at_ms INTEGER NOT NULL,
              size_bytes INTEGER NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS asset_search USING fts5(path, name);
            ",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn in_memory() -> Result<Self, LibraryError> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "
            CREATE TABLE library_roots (
              path TEXT PRIMARY KEY NOT NULL,
              added_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE TABLE assets (
              path TEXT PRIMARY KEY NOT NULL,
              name TEXT NOT NULL,
              kind TEXT NOT NULL,
              modified_at_ms INTEGER NOT NULL,
              size_bytes INTEGER NOT NULL
            );
            CREATE VIRTUAL TABLE asset_search USING fts5(path, name);
            ",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn add_root(&self, path: &Path) -> Result<(), LibraryError> {
        let canonical = path.canonicalize()?;
        self.connection.lock().execute(
            "INSERT OR IGNORE INTO library_roots(path) VALUES (?1)",
            params![canonical.to_string_lossy()],
        )?;
        Ok(())
    }

    pub fn roots(&self) -> Result<Vec<PathBuf>, LibraryError> {
        let connection = self.connection.lock();
        let mut statement =
            connection.prepare("SELECT path FROM library_roots ORDER BY added_at, path")?;
        let paths = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(paths.into_iter().map(PathBuf::from).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn stores_only_explicit_roots() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        library.add_root(root.path()).unwrap();
        assert_eq!(
            library.roots().unwrap(),
            vec![root.path().canonicalize().unwrap()]
        );
    }
}
