use super::{Library, LibraryError};
use oxy_domain::AssetSummary;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

type DirectoryKey = (PathBuf, PathBuf);

const SNAPSHOT_PERSIST_QUEUE_CAPACITY: usize = 64;

pub(super) struct DirectorySnapshots {
    slots: Mutex<HashMap<DirectoryKey, Arc<Slot>>>,
    pub background_scan: Mutex<()>,
    persistence: SnapshotPersistence,
}

struct SnapshotPersistence {
    sender: SyncSender<PersistenceMessage>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

enum PersistenceMessage {
    Persist(Arc<Slot>),
    Shutdown,
}

#[derive(Default)]
struct Slot {
    state: Mutex<SnapshotState>,
    scan: Mutex<()>,
    persistence: Mutex<PendingPersistence>,
    persistence_fence: Mutex<()>,
}

#[derive(Default)]
struct PendingPersistence {
    queued: bool,
    latest: Option<PersistenceItem>,
}

struct PersistenceItem {
    root: PathBuf,
    directory: PathBuf,
    epoch: u64,
    revision: u64,
    assets: Arc<Vec<AssetSummary>>,
}

#[derive(Default)]
struct SnapshotState {
    assets: Option<Arc<Vec<AssetSummary>>>,
    epoch: u64,
    loaded: bool,
    validated: bool,
    validated_at: Option<Instant>,
    validating: bool,
    error: Option<String>,
    revision: u64,
    previous: Option<(u64, Arc<Vec<AssetSummary>>)>,
    persistence_error: Option<String>,
}

pub struct DirectoryRead {
    pub assets: Arc<Vec<AssetSummary>>,
    pub source: &'static str,
    pub needs_validation: bool,
    pub epoch: u64,
    pub error: Option<String>,
    pub revision: u64,
    pub snapshot_serialize_ms: u64,
    pub snapshot_persist_ms: u64,
}

struct SnapshotPublish {
    published: bool,
    enqueue_ms: u64,
}

impl DirectorySnapshots {
    pub(super) fn new(connection: Arc<Mutex<Connection>>) -> Self {
        let (sender, receiver) = mpsc::sync_channel(SNAPSHOT_PERSIST_QUEUE_CAPACITY);
        let worker = thread::Builder::new()
            .name("directory-snapshot-persistence".into())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    match message {
                        PersistenceMessage::Persist(slot) => {
                            persist_pending_snapshots(&connection, &slot);
                        }
                        PersistenceMessage::Shutdown => break,
                    }
                }
            })
            .expect("directory snapshot persistence worker must start");
        Self {
            slots: Mutex::new(HashMap::new()),
            background_scan: Mutex::new(()),
            persistence: SnapshotPersistence {
                sender,
                worker: Mutex::new(Some(worker)),
            },
        }
    }

    fn enqueue(&self, slot: Arc<Slot>, item: PersistenceItem) -> Result<(), String> {
        let mut pending = slot.persistence.lock();
        pending.latest = Some(item);
        if pending.queued {
            return Ok(());
        }
        pending.queued = true;
        drop(pending);
        self.persistence
            .sender
            .send(PersistenceMessage::Persist(slot))
            .map_err(|error| error.to_string())
    }

    fn validated_assets(
        &self,
        root: &Path,
        directory: &Path,
        not_older_than: Instant,
    ) -> Option<Arc<Vec<AssetSummary>>> {
        let slot = self
            .slots
            .lock()
            .get(&(root.to_owned(), directory.to_owned()))
            .cloned()?;
        let state = slot.state.lock();
        (state.validated && state.validated_at.is_some_and(|at| at >= not_older_than))
            .then(|| state.assets.clone())
            .flatten()
    }
}

impl Drop for DirectorySnapshots {
    fn drop(&mut self) {
        let _ = self.persistence.sender.send(PersistenceMessage::Shutdown);
        if let Some(worker) = self.persistence.worker.lock().take() {
            let _ = worker.join();
        }
    }
}

fn persist_pending_snapshots(connection: &Mutex<Connection>, slot: &Slot) {
    loop {
        let item = {
            let mut pending = slot.persistence.lock();
            let Some(item) = pending.latest.take() else {
                pending.queued = false;
                return;
            };
            item
        };
        let json = match serde_json::to_string(item.assets.as_ref()) {
            Ok(json) => json,
            Err(error) => {
                record_persistence_error(slot, &item, error.to_string());
                continue;
            }
        };
        let _fence = slot.persistence_fence.lock();
        {
            let state = slot.state.lock();
            if state.epoch != item.epoch
                || state.revision != item.revision
                || state
                    .assets
                    .as_ref()
                    .is_none_or(|assets| !Arc::ptr_eq(assets, &item.assets))
            {
                continue;
            }
        }
        let result = connection.lock().execute(
            "INSERT INTO directory_snapshots(root_path,directory_path,assets_json) VALUES (?1,?2,?3)
             ON CONFLICT(root_path,directory_path) DO UPDATE SET assets_json=excluded.assets_json",
            params![
                item.root.to_string_lossy(),
                item.directory.to_string_lossy(),
                json
            ],
        );
        let mut state = slot.state.lock();
        if state.epoch == item.epoch && state.revision == item.revision {
            match result {
                Ok(_) => state.persistence_error = None,
                Err(error) => state.persistence_error = Some(error.to_string()),
            }
        }
    }
}

fn record_persistence_error(slot: &Slot, item: &PersistenceItem, error: String) {
    let mut state = slot.state.lock();
    if state.epoch == item.epoch && state.revision == item.revision {
        state.persistence_error = Some(error);
    }
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
    pub(super) fn validated_directory_assets(
        &self,
        root: &Path,
        directory: &Path,
        not_older_than: Instant,
    ) -> Option<Arc<Vec<AssetSummary>>> {
        self.directory_snapshots
            .validated_assets(root, directory, not_older_than)
    }

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
                        snapshot_serialize_ms: 0,
                        snapshot_persist_ms: 0,
                    })
                    .ok_or(LibraryError::StaleDirectorySnapshot);
            }
            if !state.loaded {
                let json: Option<String> = self.read_connection().query_row(
                    "SELECT assets_json FROM directory_snapshots WHERE root_path=?1 AND directory_path=?2",
                    params![root.to_string_lossy(), directory.to_string_lossy()], |row| row.get(0),
                ).optional()?;
                let has_snapshot_record =
                    json.is_some() || self.has_ancestor_snapshot_tombstone(root, directory)?;
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
                    snapshot_serialize_ms: 0,
                    snapshot_persist_ms: 0,
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
                    snapshot_serialize_ms: 0,
                    snapshot_persist_ms: 0,
                });
            }
            state.epoch
        };
        let assets = self.scan_browse_directory(root, directory, report)?;
        let publish = self.publish_directory(root, directory, &slot, epoch, assets.clone())?;
        if !publish.published {
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
            snapshot_serialize_ms: 0,
            snapshot_persist_ms: publish.enqueue_ms,
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
        slot: &Arc<Slot>,
        epoch: u64,
        assets: Arc<Vec<AssetSummary>>,
    ) -> Result<SnapshotPublish, LibraryError> {
        let fence = slot.persistence_fence.lock();
        let revision = {
            let mut state = slot.state.lock();
            if state.epoch != epoch {
                return Ok(SnapshotPublish {
                    published: false,
                    enqueue_ms: 0,
                });
            }
            state.previous = state.assets.take().map(|assets| (state.revision, assets));
            state.revision += 1;
            state.assets = Some(assets.clone());
            state.loaded = true;
            state.validated = true;
            state.validated_at = Some(Instant::now());
            state.validating = false;
            state.error = None;
            state.persistence_error = None;
            state.revision
        };
        drop(fence);
        let enqueue_started = std::time::Instant::now();
        if let Err(error) = self.directory_snapshots.enqueue(
            slot.clone(),
            PersistenceItem {
                root: root.to_owned(),
                directory: directory.to_owned(),
                epoch,
                revision,
                assets,
            },
        ) {
            let mut state = slot.state.lock();
            if state.epoch == epoch && state.revision == revision {
                state.persistence_error = Some(error);
            }
        }
        Ok(SnapshotPublish {
            published: true,
            enqueue_ms: enqueue_started.elapsed().as_millis() as u64,
        })
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
            .and_then(|assets| self.publish_directory(root, directory, &slot, epoch, assets))
            .map(|publish| publish.published);
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
        let matching = slots
            .iter()
            .filter(|((stored, _), _)| stored == root)
            .map(|(_, slot)| slot)
            .collect::<Vec<_>>();
        let _persistence_fences = matching
            .iter()
            .map(|slot| slot.persistence_fence.lock())
            .collect::<Vec<_>>();
        let mut states = matching
            .iter()
            .map(|slot| slot.state.lock())
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

    fn has_ancestor_snapshot_tombstone(
        &self,
        root: &Path,
        directory: &Path,
    ) -> Result<bool, LibraryError> {
        let connection = self.read_connection();
        let tombstones = connection
            .prepare(
                "SELECT directory_path FROM directory_snapshots
                 WHERE root_path=?1 AND assets_json=''",
            )?
            .query_map([root.to_string_lossy()], |row| {
                row.get::<_, String>(0).map(PathBuf::from)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tombstones
            .iter()
            .any(|tombstone| directory.starts_with(tombstone)))
    }

    /// Explicit refresh and file operations invalidate both memory and disk,
    /// including descendant snapshots when a folder was moved or deleted.
    pub fn invalidate_directory_snapshots(&self, directory: &Path) -> Result<(), LibraryError> {
        let directory = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_owned());
        let slots = self.directory_snapshots.slots.lock();
        let matching = slots
            .iter()
            .filter(|((_, path), _)| path.starts_with(&directory))
            .map(|(_, slot)| slot)
            .collect::<Vec<_>>();
        let _persistence_fences = matching
            .iter()
            .map(|slot| slot.persistence_fence.lock())
            .collect::<Vec<_>>();
        let mut states = matching
            .iter()
            .map(|slot| slot.state.lock())
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
        let registered_roots = connection
            .prepare("SELECT path FROM library_roots")?
            .query_map([], |row| row.get::<_, String>(0).map(PathBuf::from))?
            .collect::<Result<Vec<_>, _>>()?;
        paths.extend(
            registered_roots
                .into_iter()
                .filter(|root| directory.starts_with(root))
                .map(|root| (root, directory.clone())),
        );
        let transaction = connection.transaction()?;
        for (root, path) in paths {
            if path.starts_with(&directory) {
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
    fn memory_snapshot_reads_do_not_wait_for_background_sqlite_persistence() {
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        fs::write(root.join("one.jpg"), "photo").unwrap();
        let library = Library::in_memory().unwrap();
        let slot = library.directory_slot(&root, &root).unwrap();
        let assets = Arc::new(oxy_fs::scan_index_assets(&root).unwrap());
        let connection = library.connection.lock();
        library
            .publish_directory(&root, &root, &slot, 0, assets)
            .unwrap();
        let mut persistence_is_waiting = false;
        for _ in 0..100 {
            if slot.persistence_fence.try_lock().is_none() {
                persistence_is_waiting = true;
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(persistence_is_waiting);
        let (read_tx, read_rx) = mpsc::channel();
        thread::scope(|scope| {
            scope.spawn(|| {
                let read = library.browse_directory(&root, &root, |_| {}).unwrap();
                read_tx.send(read.source).unwrap();
            });
            assert_eq!(
                read_rx
                    .recv_timeout(std::time::Duration::from_secs(1))
                    .unwrap(),
                "memory"
            );
            drop(connection);
        });
    }

    #[test]
    fn background_persistence_failure_does_not_revoke_the_memory_snapshot() {
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        fs::write(root.join("one.jpg"), "photo").unwrap();
        let library = Library::in_memory().unwrap();
        library
            .connection
            .lock()
            .execute("DROP TABLE directory_snapshots", [])
            .unwrap();
        let slot = library.directory_slot(&root, &root).unwrap();
        let assets = Arc::new(oxy_fs::scan_index_assets(&root).unwrap());
        let publish = library
            .publish_directory(&root, &root, &slot, 0, assets.clone())
            .unwrap();
        assert!(publish.published);

        for _ in 0..100 {
            if slot.state.lock().persistence_error.is_some() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(slot.state.lock().persistence_error.is_some());
        let read = library.browse_directory(&root, &root, |_| {}).unwrap();
        assert!(Arc::ptr_eq(&read.assets, &assets));
        assert_eq!(read.source, "memory");
        assert!(read.error.is_none());
    }

    #[test]
    fn stale_queued_snapshot_cannot_revive_an_invalidated_record() {
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        fs::write(root.join("one.jpg"), "photo").unwrap();
        let library = Library::in_memory().unwrap();
        library
            .connection
            .lock()
            .execute(
                "INSERT INTO directory_snapshots(root_path,directory_path,assets_json) VALUES (?1,?2,'')",
                params![root.to_string_lossy(), root.to_string_lossy()],
            )
            .unwrap();
        let slot = library.directory_slot(&root, &root).unwrap();
        let assets = Arc::new(oxy_fs::scan_index_assets(&root).unwrap());
        {
            let mut state = slot.state.lock();
            state.assets = Some(assets.clone());
            state.epoch = 2;
            state.revision = 2;
        }
        {
            let mut pending = slot.persistence.lock();
            pending.queued = true;
            pending.latest = Some(PersistenceItem {
                root: root.clone(),
                directory: root.clone(),
                epoch: 1,
                revision: 1,
                assets,
            });
        }

        persist_pending_snapshots(&library.connection, &slot);

        let json: String = library
            .connection
            .lock()
            .query_row(
                "SELECT assets_json FROM directory_snapshots WHERE root_path=?1 AND directory_path=?2",
                params![root.to_string_lossy(), root.to_string_lossy()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(json.is_empty());
    }

    #[test]
    fn newer_queued_snapshot_coalesces_and_is_restart_durable() {
        let cache = tempdir().unwrap();
        let volume = tempdir().unwrap();
        let root = volume.path().join("photos");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let database = cache.path().join("library.sqlite");
        fs::write(root.join("one.jpg"), "one").unwrap();
        let first_assets = Arc::new(oxy_fs::scan_index_assets(&root).unwrap());
        fs::write(root.join("two.jpg"), "two").unwrap();
        let latest_assets = Arc::new(oxy_fs::scan_index_assets(&root).unwrap());
        let library = Library::open(&database).unwrap();
        let slot = library.directory_slot(&root, &root).unwrap();
        let mut state = slot.state.lock();
        state.assets = Some(latest_assets.clone());
        state.loaded = true;
        state.validated = true;
        state.revision = 2;
        library
            .directory_snapshots
            .enqueue(
                slot.clone(),
                PersistenceItem {
                    root: root.clone(),
                    directory: root.clone(),
                    epoch: 0,
                    revision: 1,
                    assets: first_assets,
                },
            )
            .unwrap();
        library
            .directory_snapshots
            .enqueue(
                slot.clone(),
                PersistenceItem {
                    root: root.clone(),
                    directory: root.clone(),
                    epoch: 0,
                    revision: 2,
                    assets: latest_assets,
                },
            )
            .unwrap();
        drop(state);
        drop(library);

        fs::rename(&root, volume.path().join("offline")).unwrap();
        let cached = Library::open(&database)
            .unwrap()
            .browse_directory(&root, &root, |_| panic!("must restore the queued snapshot"))
            .unwrap();
        assert_eq!(cached.assets.len(), 2);
        assert_eq!(cached.source, "snapshot");
    }

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
    fn unbrowsed_destination_tombstone_blocks_index_seed_for_its_descendants() {
        let cache = tempdir().unwrap();
        let volume = tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let destination = root.join("destination");
        let nested = destination.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("stale.jpg"), "old").unwrap();
        let database = cache.path().join("library.sqlite");
        let library = Library::open(&database).unwrap();
        library.add_root(&root).unwrap();
        library.index_root(&root).unwrap();

        fs::remove_file(nested.join("stale.jpg")).unwrap();
        fs::write(nested.join("current.jpg"), "new").unwrap();
        library
            .invalidate_directory_snapshots(&destination)
            .unwrap();
        drop(library);

        let library = Library::open(&database).unwrap();
        let tombstone: String = library
            .connection
            .lock()
            .query_row(
                "SELECT assets_json FROM directory_snapshots
                 WHERE root_path=?1 AND directory_path=?2",
                params![root.to_string_lossy(), destination.to_string_lossy()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(tombstone.is_empty());
        let read = library.browse_directory(&root, &nested, |_| {}).unwrap();
        assert_eq!(read.source, "filesystem");
        assert_eq!(read.assets.len(), 1);
        assert_eq!(read.assets[0].name, "current.jpg");
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
