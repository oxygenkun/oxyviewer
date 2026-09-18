//! One durable user-data document: version envelope, atomic replacement, and a
//! single authority for the bytes on disk.
//!
//! OxyViewer keeps a small set of facts that no rebuild can recover — a person
//! confirmation, a chosen cache location, a configured external editor. They do
//! not belong in SQLite, because SQLite is a cache this application may delete,
//! and they do not belong in memory, because closing the window is not a
//! decision to forget them.
//!
//! Every such fact lives in its own JSON file under `app_data_dir`, and every
//! one of those files needs the same four guarantees. Writing them by hand each
//! time is how a store ends up non-atomic, or silently reset by a truncated
//! file, so they live here once:
//!
//! 1. **An envelope.** The document carries a schema version, and a file from a
//!    newer build is never written: an older build cannot know which fields it
//!    would drop.
//! 2. **Atomic replacement.** [`oxy_fs::write_atomic`] leaves the old file or
//!    the new one, never a truncated mix.
//! 3. **Persist before publish.** A mutation that cannot be written must not
//!    become visible, or the caller projects a fact that is lost on restart.
//! 4. **Never destroy what was not understood.** A file this build cannot use is
//!    either moved aside ([`DocumentStore::load_recoverable`]) or left in place
//!    with writes refused. It is never quietly replaced by defaults.
//!
//! Documents stay one file each. A single combined "user data" file would turn
//! one unreadable byte into every decision at once, force one schema version on
//! unrelated features, and rewrite the whole user history on every edit.

use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

/// A user-data document that owns its own fields and the meaning of its schema
/// version.
///
/// The version is deliberately part of the document rather than a field the
/// store injects: the file stays readable and diffable on its own, and a
/// migration can rewrite fields while keeping the same envelope.
pub trait UserDocument: Clone + Default + Serialize + DeserializeOwned {
    /// Label used in diagnostics, for example `"people"`.
    const KIND: &'static str;
    /// Schema version this build writes.
    const CURRENT_VERSION: u32;

    /// Schema version read from disk.
    fn version(&self) -> u32;
    /// Marks the document as written by this build. Called before every write.
    fn stamp(&mut self);
}

/// Failures from loading or writing a document store.
///
/// None of them are user mistakes: they describe a file this build could not
/// use, which is exactly the case where guessing would cost real data.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Filesystem(#[from] oxy_fs::FsError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{kind} store {path} has unsupported version {found}; this build supports {supported}")]
    UnsupportedVersion {
        kind: &'static str,
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    /// The file was not accepted, so this build must not write it. The document
    /// is left exactly as it was found.
    #[error("refusing to write the {kind} store {path}: {reason}")]
    Refused {
        kind: &'static str,
        path: PathBuf,
        reason: String,
    },
}

/// How a store came to exist, so the caller can decide what still has to happen
/// (seeding a missing file, warning about a file that was moved aside).
///
/// [`DocumentStore::load`] only ever reports `Created` or `Loaded`; the recovery
/// variants come from [`DocumentStore::load_recoverable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreOrigin {
    /// No file existed. The document is `T::default()`.
    Created,
    /// An existing file was read and accepted.
    Loaded,
    /// The file existed but was not valid JSON. It was moved aside intact and
    /// the store starts from defaults.
    Quarantined { path: PathBuf, reason: String },
    /// The file exists and was left untouched, but this build must not write it
    /// (a newer schema, a read failure, or a document the caller rejected).
    /// Reads serve defaults; every write fails with [`StoreError::Refused`].
    Refused { reason: String },
}

/// A versioned document, kept in one file, guarded against partial writes and
/// against being overwritten by a build that does not understand it.
pub struct DocumentStore<T: UserDocument> {
    path: PathBuf,
    document: RwLock<T>,
    refusal: Option<String>,
}

impl<T: UserDocument> std::fmt::Debug for DocumentStore<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DocumentStore")
            .field("kind", &T::KIND)
            .field("path", &self.path)
            .field("refused", &self.refusal.is_some())
            .finish()
    }
}

impl<T: UserDocument> DocumentStore<T> {
    /// Loads a document strictly.
    ///
    /// A missing file is not an error: the store starts empty and reports
    /// [`StoreOrigin::Created`] so the caller can seed it. Everything else that
    /// cannot be read and accepted is an error, because the alternative — start
    /// from defaults and write them back — destroys the file it failed to
    /// understand. Only `Created` and `Loaded` can come back from here; a
    /// caller that must survive an unusable file wants
    /// [`DocumentStore::load_recoverable`].
    pub fn load(path: impl Into<PathBuf>) -> Result<(Self, StoreOrigin), StoreError> {
        let path = path.into();
        if !path.is_file() {
            return Ok((Self::new(path, T::default(), None), StoreOrigin::Created));
        }
        let document = read_document::<T>(&path)?;
        Ok((Self::new(path, document, None), StoreOrigin::Loaded))
    }

    /// Loads a preference document, which must never keep the application from
    /// starting — but must also never cost data to recover from.
    ///
    /// * A missing file starts empty and writable.
    /// * A file that cannot be parsed is moved aside intact and the store starts
    ///   from defaults, so the next write cannot erase it.
    /// * A file that cannot be read, or one from a newer build, stays exactly
    ///   where it is and the store refuses writes for this run.
    pub fn load_recoverable(path: impl Into<PathBuf>) -> (Self, StoreOrigin) {
        let path = path.into();
        if !path.is_file() {
            return (Self::new(path, T::default(), None), StoreOrigin::Created);
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                let reason = format!("the file could not be read: {error}");
                return (
                    Self::refusing(path, reason.clone()),
                    StoreOrigin::Refused { reason },
                );
            }
        };
        let document: T = match serde_json::from_slice(&bytes) {
            Ok(document) => document,
            Err(error) => {
                return match quarantine(&path) {
                    Ok(quarantined) => (
                        Self::new(path, T::default(), None),
                        StoreOrigin::Quarantined {
                            path: quarantined,
                            reason: error.to_string(),
                        },
                    ),
                    Err(move_error) => {
                        let reason = format!(
                            "the file is not valid JSON ({error}) and could not be moved aside: \
                             {move_error}"
                        );
                        (
                            Self::refusing(path, reason.clone()),
                            StoreOrigin::Refused { reason },
                        )
                    }
                };
            }
        };
        if document.version() > T::CURRENT_VERSION {
            let reason = format!(
                "the file was written by a newer build (version {}, this build supports {})",
                document.version(),
                T::CURRENT_VERSION
            );
            return (
                Self::refusing(path, reason.clone()),
                StoreOrigin::Refused { reason },
            );
        }
        (Self::new(path, document, None), StoreOrigin::Loaded)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Why this store refuses every write, if it does.
    pub fn refusal(&self) -> Option<&str> {
        self.refusal.as_deref()
    }

    /// A clone of the current document.
    pub fn read(&self) -> T {
        self.document.read().clone()
    }

    /// Reads the document without cloning it out of the lock.
    pub fn with<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        let document = self.document.read();
        read(&document)
    }

    /// Refuses every future write, leaving the file untouched.
    ///
    /// Used when the caller knows more than the envelope does: a document that
    /// parses but that this build must not act on, for example a preference file
    /// whose contents fail the caller's own validation. Reporting the reason on
    /// read is the caller's job; this only makes the refusal impossible to
    /// bypass with a later write.
    pub fn refuse_writes(&mut self, reason: impl Into<String>) {
        self.refusal = Some(reason.into());
    }

    /// Replaces the in-memory document without writing the file.
    ///
    /// This is a repair, not an edit: it exists for values this build cannot use
    /// for the current run while the file stays untouched for the build that can
    /// (an out-of-range preference, or a location on a volume that is not
    /// mounted right now). Publishing a durable change goes through
    /// [`DocumentStore::update`], which persists first.
    pub fn reset_in_memory(&mut self, document: T) {
        *self.document.get_mut() = document;
    }

    /// Applies `change` and writes the result before it becomes visible.
    ///
    /// The change is applied to a copy: if serialization or the atomic write
    /// fails, the caller gets the error and the store still holds the previous
    /// document, so nothing that was not persisted can be read back or
    /// projected.
    pub fn update<R>(&self, change: impl FnOnce(&mut T) -> R) -> Result<R, StoreError> {
        if let Some(reason) = &self.refusal {
            return Err(StoreError::Refused {
                kind: T::KIND,
                path: self.path.clone(),
                reason: reason.clone(),
            });
        }
        let mut document = self.document.write();
        let mut next = document.clone();
        let changed = change(&mut next);
        next.stamp();
        let bytes = serde_json::to_vec_pretty(&next)?;
        oxy_fs::write_atomic(&self.path, &bytes)?;
        *document = next;
        Ok(changed)
    }

    fn new(path: PathBuf, document: T, refusal: Option<String>) -> Self {
        Self {
            path,
            document: RwLock::new(document),
            refusal,
        }
    }

    /// A store that serves defaults and refuses every write.
    fn refusing(path: PathBuf, reason: String) -> Self {
        Self::new(path, T::default(), Some(reason))
    }
}

fn read_document<T: UserDocument>(path: &Path) -> Result<T, StoreError> {
    let bytes = std::fs::read(path)?;
    let document: T = serde_json::from_slice(&bytes)?;
    if document.version() > T::CURRENT_VERSION {
        return Err(StoreError::UnsupportedVersion {
            kind: T::KIND,
            path: path.to_path_buf(),
            found: document.version(),
            supported: T::CURRENT_VERSION,
        });
    }
    Ok(document)
}

/// Moves a file this build could not parse out of the way, keeping its bytes.
///
/// The name is derived from the original path and uniquified, so a second
/// corruption cannot silently replace the first quarantined file.
fn quarantine(path: &Path) -> std::io::Result<PathBuf> {
    let mut target = quarantined_path(path, None);
    let mut suffix = 1u32;
    while target.exists() {
        target = quarantined_path(path, Some(suffix));
        suffix += 1;
    }
    std::fs::rename(path, &target)?;
    Ok(target)
}

fn quarantined_path(path: &Path, suffix: Option<u32>) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    match suffix {
        Some(suffix) => name.push(format!(".corrupt-{suffix}")),
        None => name.push(".corrupt"),
    }
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Counter {
        #[serde(default)]
        version: u32,
        #[serde(default)]
        value: u32,
    }

    impl UserDocument for Counter {
        const KIND: &'static str = "counter";
        const CURRENT_VERSION: u32 = 3;

        fn version(&self) -> u32 {
            self.version
        }

        fn stamp(&mut self) {
            self.version = Self::CURRENT_VERSION;
        }
    }

    fn written(path: &Path) -> Counter {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn a_missing_file_starts_empty_and_writable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("counter.json");
        let (store, origin) = DocumentStore::<Counter>::load(&path).unwrap();
        assert_eq!(origin, StoreOrigin::Created);
        assert_eq!(store.read(), Counter::default());

        store.update(|counter| counter.value = 7).unwrap();
        assert_eq!(written(&path).value, 7);
    }

    #[test]
    fn a_write_stamps_the_current_version() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("counter.json");
        let (store, _) = DocumentStore::<Counter>::load(&path).unwrap();
        store.update(|counter| counter.value = 1).unwrap();
        assert_eq!(written(&path).version, Counter::CURRENT_VERSION);
    }

    #[test]
    fn a_newer_file_is_refused_and_left_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("counter.json");
        let original = br#"{"version": 9, "value": 41}"#;
        std::fs::write(&path, original).unwrap();

        // Strict loading is the caller's signal that nothing may be projected.
        assert!(matches!(
            DocumentStore::<Counter>::load(&path),
            Err(StoreError::UnsupportedVersion { found: 9, .. })
        ));

        let (store, origin) = DocumentStore::<Counter>::load_recoverable(&path);
        assert!(matches!(origin, StoreOrigin::Refused { .. }));
        // Reading serves defaults so the application still opens...
        assert_eq!(store.read(), Counter::default());
        // ...and the one thing that would cost data is impossible.
        assert!(matches!(
            store.update(|counter| counter.value = 1),
            Err(StoreError::Refused { .. })
        ));
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_instead_of_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("counter.json");
        std::fs::write(&path, b"{ truncated").unwrap();

        let (store, origin) = DocumentStore::<Counter>::load_recoverable(&path);
        let (quarantined, reason) = match origin {
            StoreOrigin::Quarantined { path, reason } => (path, reason),
            other => panic!("a corrupt file must be quarantined, got {other:?}"),
        };
        assert!(!reason.is_empty());
        assert_eq!(std::fs::read(&quarantined).unwrap(), b"{ truncated");

        // The store is writable again, and recovery itself wrote nothing.
        assert!(!path.exists());
        store.update(|counter| counter.value = 2).unwrap();
        assert_eq!(written(&path).value, 2);
        assert_eq!(std::fs::read(&quarantined).unwrap(), b"{ truncated");
    }

    #[test]
    fn quarantined_files_accumulate_under_distinct_names() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("counter.json");
        std::fs::write(&path, b"first broken").unwrap();
        let (_, first) = DocumentStore::<Counter>::load_recoverable(&path);
        std::fs::write(&path, b"second broken").unwrap();
        let (_, second) = DocumentStore::<Counter>::load_recoverable(&path);

        let first = match first {
            StoreOrigin::Quarantined { path, .. } => path,
            other => panic!("expected a quarantine, got {other:?}"),
        };
        let second = match second {
            StoreOrigin::Quarantined { path, .. } => path,
            other => panic!("expected a quarantine, got {other:?}"),
        };
        assert_ne!(first, second);
        assert_eq!(std::fs::read(first).unwrap(), b"first broken");
        assert_eq!(std::fs::read(second).unwrap(), b"second broken");
    }

    #[test]
    fn an_in_memory_repair_never_writes_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("counter.json");
        let (mut store, _) = DocumentStore::<Counter>::load(&path).unwrap();
        store.reset_in_memory(Counter {
            version: Counter::CURRENT_VERSION,
            value: 12,
        });
        assert_eq!(store.read().value, 12);
        assert!(
            !path.exists(),
            "a repair for this run must not create the file"
        );
    }

    #[test]
    fn a_refused_store_never_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("counter.json");
        let original = br#"{"version": 99}"#;
        std::fs::write(&path, original).unwrap();

        let (mut store, _) = DocumentStore::<Counter>::load_recoverable(&path);
        store.refuse_writes("the caller rejected this document");

        assert!(store.update(|counter| counter.value = 1).is_err());
        assert_eq!(store.refusal(), Some("the caller rejected this document"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}
