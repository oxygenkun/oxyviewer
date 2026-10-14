//! SQLite connections, WAL readers, schema registration and typed cache clearing.
//! Feature modules own their table declarations and statements; named application
//! workflows choose cross-feature transaction policy.

pub mod schema;
pub mod table;
mod vector;

use parking_lot::{Mutex, MutexGuard};
use rusqlite::OpenFlags;
use std::{path::Path, sync::Arc};
use thiserror::Error;

/// Internal handles for feature repositories and named transaction workflows.
pub use rusqlite::{Connection, Transaction};

/// Failures from opening or using the SQLite store itself.
///
/// Domain errors — a missing tag parent, a conflicting person record, an
/// invalid root order — belong to the crate that owns the rule, not here.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error(
        "unsupported SQLite schema (application {application_id:#x}, version {version}); use an isolated core data directory"
    )]
    UnsupportedSchema { application_id: i32, version: i32 },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A value that a column stores as JSON could not be encoded.
    ///
    /// The encoding belongs to the store because the column does: a caller
    /// hands over a box or a landmark set, and how it is spelled on disk is
    /// not something a domain rule should have to know.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl StoreError {
    /// Whether SQLite refused the write because a declared constraint failed.
    ///
    /// A feature policy maps this to the rule it means — a tag name repeated
    /// among its siblings, an identity claimed twice — without naming the
    /// engine, without exporting connection mechanisms through the application API.
    pub fn is_constraint_violation(&self) -> bool {
        matches!(
            self,
            Self::Sqlite(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == rusqlite::ErrorCode::ConstraintViolation
        )
    }
}

/// A SQLite file, its connections, and the schema they share.
///
/// The store creates the declared tables when it opens: the declarations and
/// the schema step live in this crate, so a storage-version bump cannot be
/// something a caller forgets to run.
pub struct Store {
    connection: Arc<Mutex<Connection>>,
    // Disk stores use WAL readers that never acquire the writer mutex.
    // Plain in-memory databases cannot share WAL; those keep `None`.
    reader: Option<Mutex<Connection>>,
    projection_reader: Option<Mutex<Connection>>,
    vector_status: Result<String, String>,
}

impl Store {
    /// Opens (or creates) a database file and brings its schema up to date.
    ///
    /// WAL and foreign keys are enabled on the write connection, the vector
    /// extension is registered before it exists, and the schema step runs
    /// user-owned tables first so a cache migration is never the first thing to
    /// define user storage.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        // A read-only preflight observes WAL state without checkpointing or
        // rewriting an unknown application's file on connection close.
        if path.metadata().is_ok_and(|metadata| metadata.len() != 0) {
            let existing = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            schema::validate(&existing)?;
        }
        let vector_registration = vector::register();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut connection = Connection::open(path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        schema::ensure(&mut connection)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let open_reader = || -> Result<Mutex<Connection>, rusqlite::Error> {
            let reader = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            Ok(Mutex::new(reader))
        };
        let reader = open_reader()?;
        let projection_reader = open_reader()?;
        let vector_status = vector_registration
            .and_then(|()| vector::probe(&connection, Some(&reader), Some(&projection_reader)));
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            reader: Some(reader),
            projection_reader: Some(projection_reader),
            vector_status,
        })
    }

    /// Opens a store that lives only in this process.
    ///
    /// There is no second connection: an in-memory database cannot be shared
    /// across handles, so [`Store::read`] answers from the write connection.
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, StoreError> {
        let vector_registration = vector::register();
        let mut connection = Connection::open_in_memory()?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        schema::ensure(&mut connection)?;
        let vector_status =
            vector_registration.and_then(|()| vector::probe(&connection, None, None));
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            reader: None,
            projection_reader: None,
            vector_status,
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
    #[cfg(test)]
    pub fn reader(&self) -> Option<&Mutex<Connection>> {
        self.reader.as_ref()
    }

    /// The projection-only reader, before locking it. `None` in memory.
    #[cfg(test)]
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

    /// The vector extension version, or why it is unavailable.
    ///
    /// Probed once at open, on every connection, so a mismatch between the
    /// writer and a reader is reported here rather than as a wrong distance
    /// much later.
    pub fn vector_status(&self) -> Result<String, String> {
        self.vector_status.clone()
    }

    /// Empties every rebuildable table. Never touches a user-owned one.
    #[cfg(test)]
    pub fn clear_rebuildable_cache(&self) -> Result<(), StoreError> {
        let mut connection = self.write();
        schema::clear_cache(&mut connection)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn refuses_an_unversioned_populated_file_without_rewriting_it() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("old.sqlite");
        {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch("CREATE TABLE user_facts(value TEXT NOT NULL); INSERT INTO user_facts VALUES('keep');").unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            Store::open(&path),
            Err(StoreError::UnsupportedSchema {
                application_id: 0,
                version: 0
            })
        ));
        assert!(
            std::fs::read(&path).unwrap() == before,
            "rejection rewrote the file"
        );
        assert!(!path.with_extension("sqlite-wal").exists());
    }

    #[test]
    fn refuses_a_future_format_without_resetting_its_user_facts() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("future.sqlite");
        {
            let store = Store::open(&path).unwrap();
            store
                .write()
                .execute(
                    "INSERT INTO library_roots(path,sort_order) VALUES('keep',0)",
                    [],
                )
                .unwrap();
            store
                .write()
                .pragma_update(None, "user_version", 99)
                .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            Store::open(&path),
            Err(StoreError::UnsupportedSchema { version: 99, .. })
        ));
        assert!(
            std::fs::read(&path).unwrap() == before,
            "rejection checkpointed the future file"
        );
    }

    #[test]
    fn refuses_interim_schema_one_without_rewriting_or_losing_user_facts() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("interim.sqlite");
        {
            let store = Store::open(&path).unwrap();
            store
                .write()
                .execute(
                    "INSERT INTO global_people(id,display_name) VALUES('keep','Ada')",
                    [],
                )
                .unwrap();
            store
                .write()
                .pragma_update(None, "user_version", 1)
                .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            Store::open(&path),
            Err(StoreError::UnsupportedSchema { version: 1, .. })
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let connection =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let name: String = connection
            .query_row(
                "SELECT display_name FROM global_people WHERE id='keep'",
                [],
                |row| row.get("display_name"),
            )
            .unwrap();
        assert_eq!(name, "Ada");
    }

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

    /// The schema step runs at open, so no caller can forget it.
    #[test]
    fn opening_creates_the_declared_schema() {
        let store = Store::in_memory().unwrap();
        let connection = store.read();

        let mut declared: Vec<&str> = schema::user_owned_tables()
            .chain(schema::rebuildable_tables())
            .collect();
        declared.sort_unstable();
        for table in &declared {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type IN ('table','view')
                     AND name = ?1) AS present",
                    [table],
                    |row| row.get("present"),
                )
                .unwrap();
            assert!(exists, "declared table {table} was not created");
        }

        // The cross-domain reference survives, which is what makes the
        // declaration order in the schema modules load-bearing rather than
        // cosmetic: a person tag link points at the user's tag vocabulary.
        let targets: Vec<String> = connection
            .prepare("PRAGMA foreign_key_list(global_person_tags)")
            .unwrap()
            .query_map([], |row| row.get("table"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            targets.contains(&"custom_tags".to_owned()),
            "global_person_tags must reference the tag vocabulary: {targets:?}"
        );
    }

    /// Re-running the schema on an existing file is what an upgrade does, so it
    /// has to be a no-op rather than a duplicate-table or duplicate-row error.
    #[test]
    fn opening_an_existing_file_reruns_the_schema_cleanly() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("store.sqlite");
        let store = Store::open(&path).unwrap();
        store
            .write()
            .execute(
                "INSERT INTO custom_tags(name, name_key, sort_order) VALUES ('Family', 'family', 0)",
                [],
            )
            .unwrap();
        drop(store);

        let store = Store::open(&path).unwrap();
        let tags: i64 = store
            .read()
            .query_row("SELECT COUNT(*) AS count FROM custom_tags", [], |row| {
                row.get("count")
            })
            .unwrap();
        assert_eq!(tags, 1, "an upgrade must not duplicate or drop a user row");
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
