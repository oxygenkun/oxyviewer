use super::{Library, LibraryError};
use oxy_domain::AssetSummary;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

type DirectoryKey = (PathBuf, PathBuf);

#[derive(Default)]
pub(super) struct DirectorySnapshots {
    slots: Mutex<HashMap<DirectoryKey, Arc<Slot>>>,
    pub background_scan: Mutex<()>,
}

#[derive(Default)]
struct Slot {
    state: Mutex<SnapshotState>,
    scan: Mutex<()>,
}

#[derive(Default)]
struct SnapshotState {
    assets: Option<Arc<Vec<AssetSummary>>>,
    epoch: u64,
    loaded: bool,
    validated: bool,
    validating: bool,
    error: Option<String>,
    revision: u64,
    previous: Option<(u64, Arc<Vec<AssetSummary>>)>,
}

pub struct DirectoryRead {
    pub assets: Arc<Vec<AssetSummary>>,
    pub source: &'static str,
    pub needs_validation: bool,
    pub epoch: u64,
    pub error: Option<String>,
    pub revision: u64,
}

pub(super) fn ensure_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS directory_snapshots (
        root_path TEXT NOT NULL, directory_path TEXT NOT NULL,
        assets_json TEXT NOT NULL, PRIMARY KEY(root_path, directory_path)
    );",
    )
}

impl Library {
    fn directory_slot(&self, root: &Path, directory: &Path) -> Result<Arc<Slot>, LibraryError> {
        if !directory.starts_with(root)
            || directory
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(oxy_fs::FsError::OutsideSessionRoot(directory.to_owned()).into());
        }
        Ok(self
            .directory_snapshots
            .slots
            .lock()
            .entry((root.to_owned(), directory.to_owned()))
            .or_default()
            .clone())
    }

    /// Reads an independently completed directory. Only a cold miss touches
    /// the volume. Concurrent requests share one scan and one in-memory list.
    pub fn browse_directory(
        &self,
        root: &Path,
        directory: &Path,
        report: impl FnMut(oxy_fs::ScanProgress),
    ) -> Result<DirectoryRead, LibraryError> {
        self.browse_directory_revision(root, directory, None, report)
    }

    pub fn browse_directory_revision(
        &self,
        root: &Path,
        directory: &Path,
        revision: Option<u64>,
        report: impl FnMut(oxy_fs::ScanProgress),
    ) -> Result<DirectoryRead, LibraryError> {
        let slot = self.directory_slot(root, directory)?;
        {
            let mut state = slot.state.lock();
            if let Some(revision) = revision {
                let assets = if revision == state.revision {
                    state.assets.clone()
                } else {
                    state
                        .previous
                        .as_ref()
                        .filter(|(old, _)| *old == revision)
                        .map(|(_, assets)| assets.clone())
                };
                return assets
                    .map(|assets| DirectoryRead {
                        assets,
                        source: "memory",
                        needs_validation: false,
                        epoch: state.epoch,
                        error: state.error.clone(),
                        revision,
                    })
                    .ok_or(LibraryError::StaleDirectorySnapshot);
            }
            if !state.loaded {
                let json: Option<String> = self.read_connection().query_row(
                    "SELECT assets_json FROM directory_snapshots WHERE root_path=?1 AND directory_path=?2",
                    params![root.to_string_lossy(), directory.to_string_lossy()], |row| row.get(0),
                ).optional()?;
                let has_snapshot_record = json.is_some();
                state.assets = json
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .map(Arc::new);
                // Upgrade existing completed indexes without forcing a NAS
                // rescan on the first launch after this cache was introduced.
                if state.assets.is_none() && !has_snapshot_record {
                    let mut reader = self.read_connection();
                    let transaction = reader.transaction()?;
                    if super::has_completed_index(&transaction, &root.to_string_lossy())? {
                        state.assets = Some(Arc::new(transaction.prepare(
                            "SELECT id,path,name,extension,kind,size_bytes,modified_at_ms,has_sidecar
                             FROM indexed_assets WHERE root_path=?1 AND parent_path=?2"
                        )?.query_map(params![root.to_string_lossy(), directory.to_string_lossy()], super::asset_from_row)?
                            .collect::<Result<Vec<_>, _>>()?));
                    }
                }
                state.loaded = true;
            }
            if let Some(assets) = state.assets.clone() {
                let needs_validation = !state.validated && !state.validating;
                state.validating |= needs_validation;
                return Ok(DirectoryRead {
                    assets,
                    source: if state.validated {
                        "memory"
                    } else {
                        "snapshot"
                    },
                    needs_validation,
                    epoch: state.epoch,
                    error: state.error.clone(),
                    revision: state.revision,
                });
            }
        }
        let _scan = slot.scan.lock();
        // Another foreground consumer may have filled the slot while waiting.
        let epoch = {
            let state = slot.state.lock();
            if let Some(assets) = state.assets.clone() {
                return Ok(DirectoryRead {
                    assets,
                    source: "memory",
                    needs_validation: false,
                    epoch: state.epoch,
                    error: state.error.clone(),
                    revision: state.revision,
                });
            }
            state.epoch
        };
        let assets = self.scan_browse_directory(root, directory, report)?;
        if !self.publish_directory(root, directory, &slot, epoch, assets.clone())? {
            return Err(LibraryError::StaleDirectorySnapshot);
        }
        let state = slot.state.lock();
        if state
            .assets
            .as_ref()
            .is_none_or(|current| !Arc::ptr_eq(current, &assets))
        {
            return Err(LibraryError::StaleDirectorySnapshot);
        }
        Ok(DirectoryRead {
            assets,
            source: "filesystem",
            needs_validation: false,
            epoch,
            error: None,
            revision: state.revision,
        })
    }

    fn scan_browse_directory(
        &self,
        root: &Path,
        directory: &Path,
        mut report: impl FnMut(oxy_fs::ScanProgress),
    ) -> Result<Arc<Vec<AssetSummary>>, LibraryError> {
        report(oxy_fs::ScanProgress {
            resolving: true,
            ..Default::default()
        });
        let started = std::time::Instant::now();
        let resolved = directory.canonicalize()?;
        let resolve_ms = started.elapsed().as_millis() as u64;
        if !resolved.starts_with(root) {
            return Err(oxy_fs::FsError::OutsideSessionRoot(resolved).into());
        }
        Ok(Arc::new(oxy_fs::scan_assets_with_progress(
            directory,
            |mut scan| {
                scan.resolve_ms = resolve_ms;
                report(scan);
            },
        )?))
    }

    fn publish_directory(
        &self,
        root: &Path,
        directory: &Path,
        slot: &Slot,
        epoch: u64,
        assets: Arc<Vec<AssetSummary>>,
    ) -> Result<bool, LibraryError> {
        let json = serde_json::to_string(assets.as_ref())?;
        let mut state = slot.state.lock();
        if state.epoch != epoch {
            return Ok(false);
        }
        self.connection.lock().execute(
            "INSERT INTO directory_snapshots(root_path,directory_path,assets_json) VALUES (?1,?2,?3)
             ON CONFLICT(root_path,directory_path) DO UPDATE SET assets_json=excluded.assets_json",
            params![root.to_string_lossy(), directory.to_string_lossy(), json],
        )?;
        state.previous = state.assets.take().map(|assets| (state.revision, assets));
        state.revision += 1;
        state.assets = Some(assets);
        state.loaded = true;
        state.validated = true;
        state.validating = false;
        state.error = None;
        Ok(true)
    }

    /// A single background directory scan at a time. Failed scans retain the
    /// last complete snapshot. Epoch fencing prevents refresh from reviving it.
    pub fn revalidate_directory(
        &self,
        root: &Path,
        directory: &Path,
        epoch: u64,
        mut report: impl FnMut(oxy_fs::ScanProgress),
    ) -> Result<bool, LibraryError> {
        let _background = self.directory_snapshots.background_scan.lock();
        self.foreground.wait_for_background();
        let slot = self.directory_slot(root, directory)?;
        if slot.state.lock().epoch != epoch {
            return Ok(false);
        }
        let result = self
            .scan_browse_directory(root, directory, |progress| {
                self.foreground.wait_for_background();
                report(progress);
            })
            .and_then(|assets| self.publish_directory(root, directory, &slot, epoch, assets));
        let mut state = slot.state.lock();
        if state.epoch != epoch {
            return Ok(false);
        }
        if state.epoch == epoch {
            state.validating = false;
            // Retry on next explicit refresh/restart rather than an offline loop.
            state.validated = true;
            state.error = result.as_ref().err().map(ToString::to_string);
        }
        result
    }

    pub fn has_directory_snapshot(
        &self,
        root: &Path,
        directory: &Path,
    ) -> Result<bool, LibraryError> {
        if self
            .directory_slot(root, directory)?
            .state
            .lock()
            .assets
            .is_some()
        {
            return Ok(true);
        }
        Ok(self.read_connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM directory_snapshots WHERE root_path=?1 AND directory_path=?2 AND assets_json!='')",
            params![root.to_string_lossy(), directory.to_string_lossy()], |row| row.get(0),
        )?)
    }

    pub(super) fn invalidate_snapshot_root(&self, root: &Path) -> Result<(), LibraryError> {
        let slots = self.directory_snapshots.slots.lock();
        let mut states = slots
            .iter()
            .filter(|((stored, _), _)| stored == root)
            .map(|(_, slot)| slot.state.lock())
            .collect::<Vec<_>>();
        for state in &mut states {
            state.epoch += 1;
            state.revision += 1;
            state.assets = None;
            state.previous = None;
            state.loaded = true;
            state.validated = false;
            state.validating = false;
            state.error = None;
        }
        self.connection.lock().execute(
            "DELETE FROM directory_snapshots WHERE root_path=?1",
            [root.to_string_lossy()],
        )?;
        Ok(())
    }

    /// Explicit refresh and file operations invalidate both memory and disk,
    /// including descendant snapshots when a folder was moved or deleted.
    pub fn invalidate_directory_snapshots(&self, directory: &Path) -> Result<(), LibraryError> {
        let slots = self.directory_snapshots.slots.lock();
        let mut states = slots
            .iter()
            .filter(|((_, path), _)| path.starts_with(directory))
            .map(|(_, slot)| slot.state.lock())
            .collect::<Vec<_>>();
        for state in &mut states {
            state.epoch += 1;
            state.assets = None;
            state.previous = None;
            state.revision += 1;
            state.loaded = true;
            state.validated = false;
            state.validating = false;
            state.error = None;
        }
        let mut connection = self.connection.lock();
        let mut paths = connection
            .prepare("SELECT root_path,directory_path FROM directory_snapshots")?
            .query_map([], |row| {
                Ok((
                    PathBuf::from(row.get::<_, String>(0)?),
                    PathBuf::from(row.get::<_, String>(1)?),
                ))
            })?
            .collect::<Result<std::collections::HashSet<_>, _>>()?;
        paths.extend(slots.keys().cloned());
        let transaction = connection.transaction()?;
        for (root, path) in paths {
            if path.starts_with(directory) {
                // A tombstone prevents a stale completed root index from
                // reseeding this explicitly invalidated directory on restart.
                transaction.execute("INSERT INTO directory_snapshots(root_path,directory_path,assets_json) VALUES (?1,?2,'')
                    ON CONFLICT(root_path,directory_path) DO UPDATE SET assets_json=''",
                    params![root.to_string_lossy(), path.to_string_lossy()])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
    };
    use tempfile::tempdir;

    #[test]
    fn completed_directory_survives_restart_without_a_root_index_or_volume() {
        let cache = tempdir().unwrap();
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let child = root.join("2026");
        fs::create_dir(&child).unwrap();
        fs::write(child.join("one.jpg"), "photo").unwrap();
        let database = cache.path().join("library.sqlite");
        let library = Library::open(&database).unwrap();
        library.add_root(&root).unwrap();
        let first = library.browse_directory(&root, &child, |_| {}).unwrap();
        assert_eq!(first.source, "filesystem");
        assert!(library.root_needs_index(&root).unwrap());
        drop(library);
        fs::rename(&child, root.join("offline")).unwrap();
        let library = Library::open(&database).unwrap();
        let cached = library
            .browse_directory(&root, &child, |_| panic!("must not scan the volume"))
            .unwrap();
        assert_eq!(cached.assets.as_ref(), first.assets.as_ref());
        assert!(cached.needs_validation);
        assert!(
            library
                .revalidate_directory(&root, &child, cached.epoch, |_| {})
                .is_err()
        );
        let retained = library.browse_directory(&root, &child, |_| {}).unwrap();
        assert_eq!(retained.assets.len(), 1);
        assert!(retained.error.is_some());
        assert!(!retained.needs_validation);
    }

    #[test]
    fn empty_directory_is_a_complete_snapshot() {
        let cache = tempdir().unwrap();
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let database = cache.path().join("library.sqlite");
        Library::open(&database)
            .unwrap()
            .browse_directory(&root, &root, |_| {})
            .unwrap();
        let read = Library::open(&database)
            .unwrap()
            .browse_directory(&root, &root, |_| panic!("empty is cached"))
            .unwrap();
        assert!(read.assets.is_empty());
        assert_eq!(read.source, "snapshot");
    }

    #[test]
    fn successful_validation_atomically_replaces_added_deleted_and_sidecar_state() {
        let cache = tempdir().unwrap();
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let database = cache.path().join("library.sqlite");
        fs::write(root.join("old.jpg"), "old").unwrap();
        Library::open(&database)
            .unwrap()
            .browse_directory(&root, &root, |_| {})
            .unwrap();
        fs::remove_file(root.join("old.jpg")).unwrap();
        fs::write(root.join("new.jpg"), "new").unwrap();
        fs::write(root.join("new.xmp"), "sidecar").unwrap();
        let library = Library::open(&database).unwrap();
        let read = library.browse_directory(&root, &root, |_| {}).unwrap();
        assert_eq!(read.assets[0].name, "old.jpg");
        assert!(
            library
                .revalidate_directory(&root, &root, read.epoch, |_| {})
                .unwrap()
        );
        let updated = library.browse_directory(&root, &root, |_| {}).unwrap();
        assert_eq!(updated.assets.len(), 1);
        assert_eq!(updated.assets[0].name, "new.jpg");
        assert!(updated.assets[0].has_sidecar);
        assert!(!updated.needs_validation);
    }

    #[test]
    fn concurrent_cold_requests_share_one_scan() {
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let library = Library::in_memory().unwrap();
        let scans = AtomicUsize::new(0);
        thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    library
                        .browse_directory(&root, &root, |progress| {
                            if !progress.reading_attributes && !progress.resolving {
                                scans.fetch_add(1, Ordering::Relaxed);
                            }
                        })
                        .unwrap()
                });
            }
        });
        assert_eq!(scans.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn refresh_fences_a_running_background_scan_without_blocking_foreground() {
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let library = Library::in_memory().unwrap();
        let first = library.browse_directory(&root, &root, |_| {}).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        thread::scope(|scope| {
            let library_ref = &library;
            let root_ref = &root;
            let background = scope.spawn(move || {
                let mut first_callback = true;
                library_ref
                    .revalidate_directory(root_ref, root_ref, first.epoch, |_| {
                        if first_callback {
                            first_callback = false;
                            entered_tx.send(()).unwrap();
                            resume_rx.recv().unwrap();
                        }
                    })
                    .unwrap()
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            library.invalidate_directory_snapshots(&root).unwrap();
            let _foreground = library.foreground.enter();
            library.browse_directory(&root, &root, |_| {}).unwrap();
            drop(_foreground);
            resume_tx.send(()).unwrap();
            assert!(!background.join().unwrap());
        });
    }

    #[test]
    fn old_completed_index_can_seed_browsing_but_not_after_explicit_invalidation() {
        let cache = tempdir().unwrap();
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let database = cache.path().join("library.sqlite");
        fs::write(root.join("old.jpg"), "old").unwrap();
        let library = Library::open(&database).unwrap();
        library.add_root(&root).unwrap();
        library.index_root(&root).unwrap();
        let read = library
            .browse_directory(&root, &root, |_| panic!("reuse existing index"))
            .unwrap();
        assert_eq!(read.assets.len(), 1);
        library.invalidate_directory_snapshots(&root).unwrap();
        drop(library);
        fs::remove_file(root.join("old.jpg")).unwrap();
        let read = Library::open(&database)
            .unwrap()
            .browse_directory(&root, &root, |_| {})
            .unwrap();
        assert!(read.assets.is_empty());
        assert_eq!(read.source, "filesystem");
    }

    #[test]
    fn browse_cache_does_not_wait_for_an_index_generation() {
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let library = Library::in_memory().unwrap();
        library.add_root(&root).unwrap();
        let _index = library.index_gate.lock();
        assert!(
            library
                .browse_directory(&root, &root, |_| {})
                .unwrap()
                .assets
                .is_empty()
        );
        assert!(library.root_needs_index(&root).unwrap());
    }

    #[test]
    fn cached_paths_cannot_escape_the_session_root() {
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let library = Library::in_memory().unwrap();
        assert!(
            library
                .browse_directory(&root, &root.join("../outside"), |_| {})
                .is_err()
        );
        assert!(
            library
                .browse_directory(&root, root.parent().unwrap(), |_| {})
                .is_err()
        );
    }

    #[test]
    fn removing_parent_root_preserves_child_root_snapshot() {
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let child = root.join("child");
        fs::create_dir(&child).unwrap();
        let library = Library::in_memory().unwrap();
        library.add_root(&root).unwrap();
        library.add_root(&child).unwrap();
        library.browse_directory(&root, &child, |_| {}).unwrap();
        library.browse_directory(&child, &child, |_| {}).unwrap();
        library.remove_root(&root).unwrap();
        assert!(!library.has_directory_snapshot(&root, &child).unwrap());
        assert!(library.has_directory_snapshot(&child, &child).unwrap());
        assert_eq!(
            library
                .browse_directory(&child, &child, |_| panic!("child remains cached"))
                .unwrap()
                .source,
            "memory"
        );
    }

    #[test]
    fn paging_keeps_the_previous_snapshot_when_validation_inserts_an_earlier_file() {
        let cache = tempdir().unwrap();
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let database = cache.path().join("library.sqlite");
        for name in ["b.jpg", "c.jpg"] {
            fs::write(root.join(name), "photo").unwrap();
        }
        Library::open(&database)
            .unwrap()
            .browse_directory(&root, &root, |_| {})
            .unwrap();
        let library = Library::open(&database).unwrap();
        let original = library.browse_directory(&root, &root, |_| {}).unwrap();
        let query = oxy_domain::AssetQuery {
            page_size: Some(1),
            ..Default::default()
        };
        assert_eq!(
            oxy_fs::page_assets(&original.assets, &query, 0).items[0].name,
            "b.jpg"
        );
        fs::write(root.join("a.jpg"), "new").unwrap();
        library
            .revalidate_directory(&root, &root, original.epoch, |_| {})
            .unwrap();
        let next = library
            .browse_directory_revision(&root, &root, Some(original.revision), |_| {})
            .unwrap();
        assert_eq!(
            oxy_fs::page_assets(&next.assets, &query, 1).items[0].name,
            "c.jpg"
        );
        let latest = library.browse_directory(&root, &root, |_| {}).unwrap();
        assert_ne!(latest.revision, original.revision);
        assert_eq!(
            oxy_fs::page_assets(&latest.assets, &query, 0).items[0].name,
            "a.jpg"
        );
        library.invalidate_directory_snapshots(&root).unwrap();
        assert!(matches!(
            library.browse_directory_revision(&root, &root, Some(original.revision), |_| {}),
            Err(LibraryError::StaleDirectorySnapshot)
        ));
    }
}
