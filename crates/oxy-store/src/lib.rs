//! The local SQLite store: connections, transactions, and table declarations.
//!
//! This crate knows how to open, share, and read a SQLite file. It knows
//! nothing about photos, tags, or people — no table here belongs to a domain.
//! A domain crate declares its own tables with [`table::tables!`] and calls
//! [`table::create_all`] from its own schema step, so the storage mechanism and
//! the meaning of the data stay in separate crates.
//!
//! The one structural decision this crate does make is the *reader fan-out*:
//! a disk store keeps read-only WAL connections so a foreground query never
//! waits for a background index write. [`Store::read`] falls back to the write
//! connection for in-memory stores, which cannot share WAL.
//!
//! Table ownership, cache clearing, and the "rebuildable vs user-owned" split
//! are *not* here. Those are domain policy: the crate that declares a table
//! decides whether a cache clear may empty it.

pub mod table;

use parking_lot::{Mutex, MutexGuard};
use rusqlite::{Connection, OpenFlags};
use std::{path::Path, sync::Arc};
use thiserror::Error;

/// Failures from opening or using the SQLite store itself.
///
/// Domain errors — a missing tag parent, a conflicting person record, an
/// invalid root order — belong to the domain crate, not here.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A SQLite file and the connections that read and write it.
///
/// The store is schema-agnostic: it opens the file, enables the pragmas every
/// connection in this application needs, and hands out connections. Creating
/// tables is the caller's job, in the caller's order.
pub struct Store {
    connection: Arc<Mutex<Connection>>,
    // Disk stores use WAL readers that never acquire the writer mutex.
    // Plain in-memory databases cannot share WAL; those keep `None`.
    reader: Option<Mutex<Connection>>,
    projection_reader: Option<Mutex<Connection>>,
}

impl Store {
    /// Opens (or creates) a database file with WAL and foreign keys enabled.
    ///
    /// No table is created here. The caller runs its own schema step on the
    /// write connection, so a storage version bump can never be the first
    /// thing to define domain storage.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let open_reader = || -> Result<Mutex<Connection>, rusqlite::Error> {
            let reader = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            Ok(Mutex::new(reader))
        };
        let reader = open_reader()?;
        let projection_reader = open_reader()?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            reader: Some(reader),
            projection_reader: Some(projection_reader),
        })
    }

    /// Opens a store that lives only in this process.
    ///
    /// There is no second connection: an in-memory database cannot be shared
    /// across handles, so [`Store::read`] answers from the write connection.
    pub fn in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            reader: None,
            projection_reader: None,
        })
    }

    /// The write connection. Hold it only for the duration of a write.
    pub fn write(&self) -> MutexGuard<'_, Connection> {
        self.connection.lock()
    }

    /// A read connection that never waits for a background index write.
    pub fn read(&self) -> MutexGuard<'_, Connection> {
        self.reader.as_ref().unwrap_or(&self.connection).lock()
    }

    /// A read connection reserved for resource projections, so a projection
    /// read does not queue behind browsing reads.
    pub fn read_projection(&self) -> MutexGuard<'_, Connection> {
        self.projection_reader
            .as_ref()
            .unwrap_or(&self.connection)
            .lock()
    }

    /// The read-only WAL reader, before locking it. `None` in memory.
    ///
    /// Extension probing needs the connection handle itself rather than a
    /// guard, because it opens a second statement on each connection.
    pub fn reader(&self) -> Option<&Mutex<Connection>> {
        self.reader.as_ref()
    }

    /// The projection-only reader, before locking it. `None` in memory.
    pub fn projection_reader(&self) -> Option<&Mutex<Connection>> {
        self.projection_reader.as_ref()
    }

    /// The shared write connection, for workers that own their own locking.
    ///
    /// A background worker that persists snapshots keeps this handle and locks
    /// it per job instead of holding the write connection for its whole life.
    pub fn shared_connection(&self) -> Arc<Mutex<Connection>> {
        self.connection.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn a_disk_store_keeps_separate_readers() {
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("store.sqlite")).unwrap();
        assert!(store.reader().is_some());
        assert!(store.projection_reader().is_some());
    }

    #[test]
    fn an_in_memory_store_shares_one_connection() {
        let store = Store::in_memory().unwrap();
        assert!(store.reader().is_none());
        assert!(store.projection_reader().is_none());
        store
            .write()
            .execute("CREATE TABLE probe(id INTEGER)", [])
            .unwrap();
        let count: i64 = store
            .read()
            .query_row("SELECT COUNT(*) AS count FROM probe", [], |row| {
                row.get("count")
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn opening_creates_no_tables_of_its_own() {
        let store = Store::in_memory().unwrap();
        let tables: i64 = store
            .read()
            .query_row(
                "SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table'
                 AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get("count"),
            )
            .unwrap();
        assert_eq!(tables, 0, "the store must not define domain storage");
    }

    #[test]
    fn write_and_read_guards_are_distinct_on_disk() {
        let directory = tempdir().unwrap();
        let store = Store::open(&directory.path().join("store.sqlite")).unwrap();
        let writer = store.write();
        // A WAL reader must not wait for a held writer, which is the property
        // the read/write split exists to provide.
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _read = store.read();
                sender.send(()).unwrap();
            });
            assert!(
                receiver
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .is_ok(),
                "a WAL read waited for the writer"
            );
        });
        drop(writer);
    }
}
