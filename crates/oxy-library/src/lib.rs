//! The local photo library: the roots the user added and the derived state
//! over one SQLite store.
//!
//! The store — the file, the connections, the table declarations, and the
//! migrations — lives in [`oxy_store`]. This crate owns what the rows *mean*,
//! and it splits them by ownership: [`cache`] holds derived state that can
//! always be rebuilt from the original photos, and [`user`] holds facts the
//! user typed, named, confirmed, or assigned. Table classes, the list of
//! rebuildable tables, and the cache-clearing entry point are declared where
//! the tables are, in `oxy_store::schema`; what stays here is every rule about
//! what the data means and the source-level audit that the two namespaces do
//! not write across their boundary.
//!
//! It owns neither the tag vocabulary nor the person domain. Those are
//! `oxy-tags` and `oxy-people` — siblings over the same store, because domain
//! rules must not be able to name each other and only crates enforce that.
//! Both were carved out of this crate's `user` namespace; what remains under
//! `user` is the favourites, and what remains under `cache` is browsing,
//! indexing, and resource projections.
//!
//! The split is a safety property, not just layout: a cache migration, a
//! preview clear, or a re-index must never be able to reach a person identity,
//! a review decision, a tag, or an explicit root.

mod cache;
pub use cache::browsing::DirectoryRead;
pub use cache::index::{IndexProgress, IndexStage, IndexStats};
#[cfg(test)]
mod audit;
mod relocation;
mod user;

use oxy_store::Store;
use parking_lot::{Mutex, MutexGuard};
use rusqlite::Connection;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;

pub(crate) const DEFAULT_PAGE_SIZE: usize = 250;
pub(crate) const MAX_PAGE_SIZE: usize = 1_000;
pub(crate) const INDEX_WRITE_BATCH_SIZE: usize = 256;
pub(crate) const INDEX_WRITE_TIME_SLICE: Duration = Duration::from_millis(8);

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("directory snapshot changed; reload the first page")]
    StaleDirectorySnapshot,
    #[error(transparent)]
    Store(#[from] oxy_store::StoreError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Filesystem(#[from] oxy_fs::FsError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("folder order must contain every library root exactly once")]
    InvalidRootOrder,
    #[error("cannot relocate folder: {0}")]
    InvalidRelocation(String),
}

/// The photo library: the roots the user added and the derived state over one
/// SQLite store.
///
/// The storage mechanism lives in [`oxy_store`]; this crate owns what the data
/// *means*. It holds the [`Store`], the in-memory directory snapshots, the
/// rebuildable index bookkeeping, and the schedule that decides whether a scan
/// may run — and nothing about the file format, the WAL setup, or the pragmas.
///
/// The store is shared rather than owned: the application opens the file once
/// and gives it to every domain crate, so none of them wraps the others. Tags
/// live in [`oxy_tags`] and people in `oxy_people`, which is why nothing named
/// `tag` and nothing named `person` is left here.
pub struct Library {
    store: Arc<Store>,
    directory_snapshots: cache::browsing::DirectorySnapshots,
    pub foreground: oxy_runtime::ForegroundGate,
    indexing_roots: Mutex<HashSet<PathBuf>>,
    index_gate: Mutex<()>,
}

impl Library {
    /// The write connection. Domain modules hold it for the duration of a write.
    fn write(&self) -> MutexGuard<'_, Connection> {
        self.store.write()
    }

    /// A read connection that never waits for a background index write.
    fn read_connection(&self) -> MutexGuard<'_, Connection> {
        self.store.read()
    }

    /// A read connection reserved for resource projections.
    fn read_projection_connection(&self) -> MutexGuard<'_, Connection> {
        self.store.read_projection()
    }

    /// The shared write connection, for tests that drive the snapshot
    /// persistence worker directly.
    #[cfg(test)]
    fn shared_connection(&self) -> Arc<Mutex<Connection>> {
        self.store.shared_connection()
    }

    /// Opens the library over the store at `path`.
    ///
    /// The store creates the file, enables WAL, registers the vector extension,
    /// and brings the schema up to date with user-owned tables first. Nothing
    /// about the schema is decided here, so a storage version bump cannot be
    /// something this constructor forgets to run.
    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        Ok(Self::with_store(Arc::new(Store::open(path)?)))
    }

    /// Opens a library that lives only in this process.
    pub fn in_memory() -> Result<Self, LibraryError> {
        Ok(Self::with_store(Arc::new(Store::in_memory()?)))
    }

    /// Builds the library over a store the caller already holds.
    ///
    /// This is the constructor a composition root uses: one file, one
    /// [`Store`], and one handle per domain crate over it. [`Library::open`]
    /// remains the shorter form for a caller that owns the file alone.
    pub fn with_store(store: Arc<Store>) -> Self {
        let directory_snapshots =
            cache::browsing::DirectorySnapshots::new(store.shared_connection());
        Self {
            store,
            directory_snapshots,
            indexing_roots: Mutex::new(HashSet::new()),
            index_gate: Mutex::new(()),
            foreground: oxy_runtime::ForegroundGate::default(),
        }
    }

    /// The store this library reads, so a sibling domain can be built over it.
    pub fn store(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_domain::{
        AssetKind, AssetQuery, AssetSort, AssetSummary, ImageProjection, MetadataProjection,
        PickLabel, RenderLevel, ResourceLoadStatus, SortDirection,
    };
    use rusqlite::params;
    use std::time::Instant;
    use tempfile::tempdir;

    fn query(search: Option<&str>) -> AssetQuery {
        AssetQuery {
            search: search.map(str::to_owned),
            sort: AssetSort::Name,
            direction: SortDirection::Ascending,
            page_size: Some(10),
            ..AssetQuery::default()
        }
    }

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

    #[test]
    fn preserves_import_order_and_allows_explicit_reordering() {
        let library = Library::in_memory().unwrap();
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        let third = tempdir().unwrap();
        library.add_root(second.path()).unwrap();
        library.add_root(first.path()).unwrap();
        library.add_root(third.path()).unwrap();

        let expected =
            [third.path(), second.path(), first.path()].map(|path| path.canonicalize().unwrap());
        library.reorder_roots(&expected).unwrap();
        assert_eq!(library.roots().unwrap(), expected);
    }

    #[test]
    fn rejects_incomplete_root_orders() {
        let library = Library::in_memory().unwrap();
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        library.add_root(first.path()).unwrap();
        library.add_root(second.path()).unwrap();

        assert!(matches!(
            library.reorder_roots(&[first.path().canonicalize().unwrap()]),
            Err(LibraryError::InvalidRootOrder)
        ));
    }

    #[test]
    fn migrates_legacy_roots_and_persists_reordering() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("library.sqlite");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        {
            let legacy = Connection::open(&database).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE library_roots (
                       path TEXT PRIMARY KEY NOT NULL,
                       added_at INTEGER NOT NULL DEFAULT (unixepoch())
                     );",
                )
                .unwrap();
            legacy
                .execute(
                    "INSERT INTO library_roots(path, added_at) VALUES (?1, 1)",
                    params![first.to_string_lossy()],
                )
                .unwrap();
            legacy
                .execute(
                    "INSERT INTO library_roots(path, added_at) VALUES (?1, 2)",
                    params![second.to_string_lossy()],
                )
                .unwrap();
        }

        let library = Library::open(&database).unwrap();
        assert_eq!(library.roots().unwrap(), [first.clone(), second.clone()]);
        library
            .reorder_roots(&[second.clone(), first.clone()])
            .unwrap();
        drop(library);

        let reopened = Library::open(&database).unwrap();
        assert_eq!(reopened.roots().unwrap(), [second, first]);
    }

    #[test]
    fn migrates_resource_projection_state_revision_and_pick_labels() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("library.sqlite");
        {
            let legacy = Connection::open(&database).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE resource_projections (
                       path TEXT NOT NULL,
                       parent_path TEXT NOT NULL,
                       projection_kind TEXT NOT NULL,
                       source_revision TEXT NOT NULL,
                       valid_at INTEGER NOT NULL,
                       projection_revision INTEGER NOT NULL,
                       status TEXT NOT NULL,
                       rating INTEGER,
                       color_label TEXT,
                       result_json TEXT,
                       error TEXT,
                       PRIMARY KEY(path, projection_kind)
                     );
                     INSERT INTO resource_projections(
                       path, parent_path, projection_kind, source_revision, valid_at,
                       projection_revision, status, rating, color_label, result_json, error
                     ) VALUES (
                       'legacy.jpg', '', 'metadata', 'legacy-source', 7,
                       11, 'ready', 4, 'Red', NULL, NULL
                     );",
                )
                .unwrap();
        }

        let library = Library::open(&database).unwrap();
        let columns = library
            .write()
            .prepare("PRAGMA table_info(resource_projections)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>("name"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "pick_label"));
        assert!(columns.iter().any(|column| column == "state_revision"));
        assert!(!columns.iter().any(|column| column == "projection_revision"));
        let migrated = library
            .metadata_projection(Path::new("legacy.jpg"), "legacy-source")
            .unwrap()
            .unwrap();
        assert_eq!(migrated.state_revision, 11);
        assert_eq!(migrated.rating, Some(4));
    }

    #[test]
    fn stores_parent_and_child_as_independent_roots_and_removes_one() {
        let library = Library::in_memory().unwrap();
        let parent = tempdir().unwrap();
        let child = parent.path().join("child");
        std::fs::create_dir(&child).unwrap();

        library.add_root(parent.path()).unwrap();
        library.add_root(&child).unwrap();
        assert_eq!(library.roots().unwrap().len(), 2);

        library.remove_root(&child).unwrap();
        assert_eq!(
            library.roots().unwrap(),
            vec![parent.path().canonicalize().unwrap()]
        );
    }

    #[test]
    fn indexes_directory_batches_and_searches_names_and_folder_paths() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let coast = root.path().join("Trips").join("Coast");
        std::fs::create_dir_all(&coast).unwrap();
        std::fs::write(root.path().join("portrait.jpg"), b"jpeg").unwrap();
        std::fs::write(coast.join("sunrise.ARW"), b"raw").unwrap();
        library.add_root(root.path()).unwrap();

        let stats = library.index_root(root.path()).unwrap().unwrap();

        assert_eq!(stats.asset_count, 2);
        assert_eq!(stats.directory_count, 2);
        let canonical_root = root.path().canonicalize().unwrap();
        let root_page = library
            .list_assets(&canonical_root, &canonical_root, &query(None), 0)
            .unwrap()
            .unwrap();
        assert_eq!(root_page.items.len(), 1);
        assert_eq!(root_page.items[0].name, "portrait.jpg");

        let name_results = library
            .list_assets(&canonical_root, &canonical_root, &query(Some("sun")), 0)
            .unwrap()
            .unwrap();
        assert_eq!(name_results.items[0].name, "sunrise.ARW");
        let folder_results = library
            .list_assets(&canonical_root, &canonical_root, &query(Some("Coast")), 0)
            .unwrap()
            .unwrap();
        assert_eq!(folder_results.items[0].name, "sunrise.ARW");
        let punctuation_results = library
            .list_assets(&canonical_root, &canonical_root, &query(Some("\"")), 0)
            .unwrap()
            .unwrap();
        assert_eq!(punctuation_results.total, 0);

        let directories = library
            .list_directories(&canonical_root, &canonical_root)
            .unwrap()
            .unwrap();
        assert_eq!(directories[0].name, "Trips");
        assert!(directories[0].has_children);

        let directory_matches = library
            .search_directories(&canonical_root, "coa")
            .unwrap()
            .unwrap();
        assert_eq!(directory_matches.len(), 1);
        assert_eq!(directory_matches[0].directory.name, "Coast");
        assert!(!directory_matches[0].directory.has_children);
        assert_eq!(directory_matches[0].ancestors.len(), 1);
        assert_eq!(directory_matches[0].ancestors[0].name, "Trips");
        assert!(
            library
                .search_directories(&canonical_root, "Trips/Coast")
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn completed_index_is_reused_until_explicitly_invalidated() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        library.add_root(root.path()).unwrap();

        assert!(library.root_needs_index(root.path()).unwrap());
        library.index_root(root.path()).unwrap();
        assert!(!library.root_needs_index(root.path()).unwrap());

        library.invalidate_index(root.path()).unwrap();
        assert!(library.root_needs_index(root.path()).unwrap());
    }

    #[test]
    fn wal_reads_finish_during_an_uncommitted_index_write() {
        let state = tempdir().unwrap();
        let library = Library::open(&state.path().join("library.sqlite")).unwrap();
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::write(root.join("photo.jpg"), b"jpeg").unwrap();
        library.add_root(&root).unwrap();
        library.index_root(&root).unwrap();

        let (sender, receiver) = std::sync::mpsc::channel();
        let completed = std::thread::scope(|scope| {
            let mut writer = library.write();
            let transaction = writer.transaction().unwrap();
            transaction
                .execute("UPDATE indexed_assets SET name = 'uncommitted.jpg'", [])
                .unwrap();
            scope.spawn(|| {
                let page = library
                    .list_assets(&root, &root, &query(None), 0)
                    .unwrap()
                    .unwrap();
                assert_eq!(page.items[0].name, "photo.jpg");
                assert!(library.contains_root(&root).unwrap());
                assert!(!library.root_needs_index(&root).unwrap());
                assert_eq!(library.roots().unwrap(), vec![root.clone()]);
                assert!(library.list_directories(&root, &root).unwrap().is_some());
                assert!(
                    library
                        .search_directories(&root, "photo")
                        .unwrap()
                        .is_some()
                );
                assert!(
                    oxy_store::repo::tags::list_tags(&library.store().read())
                        .unwrap()
                        .is_empty()
                );
                sender.send(()).unwrap();
            });
            let completed = receiver.recv_timeout(Duration::from_secs(2));
            // Always release before joining, so a lock regression fails instead of hanging.
            transaction.commit().unwrap();
            drop(writer);
            completed
        });
        assert!(completed.is_ok(), "WAL reader waited for the index writer");
        let page = library
            .list_assets(&root, &root, &query(None), 0)
            .unwrap()
            .unwrap();
        assert_eq!(page.items[0].name, "uncommitted.jpg");
    }

    #[test]
    fn projection_reads_do_not_wait_for_browsing_or_the_writer() {
        let state = tempdir().unwrap();
        let library = Library::open(&state.path().join("library.sqlite")).unwrap();
        let path = state.path().join("photo.jpg");
        let valid_at = library.next_resource_revision().unwrap();
        library
            .accept_metadata_projection(MetadataProjection {
                path: path.clone(),
                source_revision: "100:200:xmp-a".into(),
                state_revision: 0,
                valid_at,
                status: ResourceLoadStatus::Ready,
                rating: Some(4),
                color_label: None,
                pick_label: None,
                error: None,
            })
            .unwrap();
        let writer = library.write();
        let browsing = library.read_connection();
        let (sender, receiver) = std::sync::mpsc::channel();
        let completed = std::thread::scope(|scope| {
            scope.spawn(|| {
                assert_eq!(
                    library
                        .metadata_projection(&path, "100:200:xmp-a")
                        .unwrap()
                        .unwrap()
                        .rating,
                    Some(4)
                );
                assert!(
                    library
                        .image_projection(&path, RenderLevel::Thumbnail, "missing")
                        .unwrap()
                        .is_none()
                );
                sender.send(()).unwrap();
            });
            let completed = receiver.recv_timeout(Duration::from_secs(2));
            drop(browsing);
            drop(writer);
            completed
        });
        assert!(
            completed.is_ok(),
            "projection reader waited for browsing or indexing"
        );
    }

    #[test]
    #[ignore = "manual WAL contention benchmark; run with --ignored --nocapture"]
    fn wal_interaction_latency_during_large_index() {
        let state = tempdir().unwrap();
        let library = Library::open(&state.path().join("library.sqlite")).unwrap();
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::write(root.join("visible.jpg"), b"jpeg").unwrap();
        library.add_root(&root).unwrap();
        library.index_root(&root).unwrap();
        let template = oxy_fs::scan_index_assets(&root).unwrap().remove(0);
        let background = root.join("background");
        let assets: Vec<_> = (0..50_000)
            .map(|index| {
                let mut asset = template.clone();
                asset.name = format!("photo-{index}.jpg");
                asset.path = background.join(&asset.name);
                asset.id = format!("asset-{index}");
                asset
            })
            .collect();
        let scan_id = library.next_scan_id().unwrap();
        let start = std::sync::Barrier::new(2);
        let mut reads = Vec::new();
        let mut writes = Vec::new();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                start.wait();
                library
                    .write_asset_index_batch(&root, &background, scan_id, &assets)
                    .unwrap();
            });
            start.wait();
            for _ in 0..200 {
                let started = Instant::now();
                let page = library
                    .list_assets(&root, &root, &query(None), 0)
                    .unwrap()
                    .unwrap();
                assert_eq!(page.items.len(), 1);
                library
                    .metadata_projection(&template.path, "missing")
                    .unwrap();
                library
                    .image_projection(&template.path, RenderLevel::Thumbnail, "missing")
                    .unwrap();
                reads.push(started.elapsed());
                let started = Instant::now();
                library.next_resource_revision().unwrap();
                writes.push(started.elapsed());
            }
        });
        reads.sort_unstable();
        writes.sort_unstable();
        println!(
            "50k background writes, 200 foreground samples: read P95 {:?}, max {:?}; revision write P95 {:?}, max {:?}",
            reads[189], reads[199], writes[189], writes[199]
        );
    }

    #[test]
    #[ignore = "manual large-library indexing benchmark; run with --ignored --nocapture"]
    fn large_library_index_batch_latency() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        library.add_root(&root).unwrap();
        std::fs::write(root.join("template.jpg"), b"jpeg").unwrap();
        let template = oxy_fs::scan_index_assets(&root).unwrap().remove(0);
        let scan_id = library.next_scan_id().unwrap();
        let mut inserted = 0;
        for target in [1_000, 10_000, 50_000] {
            while inserted < target {
                let count = INDEX_WRITE_BATCH_SIZE.min(target - inserted);
                let assets: Vec<_> = (inserted..inserted + count)
                    .map(|index| {
                        let mut asset = template.clone();
                        asset.name = format!("photo-{index}.jpg");
                        asset.path = root.join(&asset.name);
                        asset.id = format!("asset-{index}");
                        asset
                    })
                    .collect();
                library
                    .write_asset_index_batch(&root, &root, scan_id, &assets)
                    .unwrap();
                inserted += count;
            }
            let assets: Vec<_> = (inserted..inserted + INDEX_WRITE_BATCH_SIZE)
                .map(|index| {
                    let mut asset = template.clone();
                    asset.name = format!("photo-{index}.jpg");
                    asset.path = root.join(&asset.name);
                    asset.id = format!("asset-{index}");
                    asset
                })
                .collect();
            let start = std::time::Instant::now();
            library
                .write_asset_index_batch(&root, &root, scan_id, &assets)
                .unwrap();
            println!(
                "{target} indexed assets: next {INDEX_WRITE_BATCH_SIZE} writes took {:?}",
                start.elapsed()
            );
            inserted += assets.len();
        }
        let connection = library.write();
        let count: usize = connection
            .query_row(
                "SELECT COUNT(*) AS count FROM indexed_asset_search",
                [],
                |row| row.get("count"),
            )
            .unwrap();
        assert_eq!(count, inserted);
    }

    #[test]
    fn migrates_search_keys_and_replaces_changed_terms_without_duplicates() {
        let state = tempdir().unwrap();
        let database = state.path().join("library.sqlite");
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::write(root.join("photo.jpg"), b"jpeg").unwrap();
        let library = Library::open(&database).unwrap();
        library.add_root(&root).unwrap();
        library.index_root(&root).unwrap();
        // Simulate the pre-migration database, including a non-default FTS rowid.
        library
            .write()
            .execute_batch(
                "UPDATE indexed_asset_search SET rowid = 1234;
             DROP TABLE indexed_asset_search_keys;",
            )
            .unwrap();
        drop(library);

        let library = Library::open(&database).unwrap();
        let mut assets = oxy_fs::scan_index_assets(&root).unwrap();
        assets[0].name = "replacement.jpg".into();
        let scan_id = library.next_scan_id().unwrap();
        library
            .write_asset_index_batch(&root, &root, scan_id, &assets)
            .unwrap();
        library.finish_index(&root, scan_id, 1, 0).unwrap();
        assert_eq!(
            library
                .list_assets(&root, &root, &query(Some("photo")), 0)
                .unwrap()
                .unwrap()
                .total,
            0
        );
        assert_eq!(
            library
                .list_assets(&root, &root, &query(Some("replacement")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
        let connection = library.write();
        let (count, rowid): (usize, i64) = connection
            .query_row(
                "SELECT COUNT(*) AS count, MIN(rowid) AS min_rowid FROM indexed_asset_search",
                [],
                |row| Ok((row.get("count")?, row.get("min_rowid")?)),
            )
            .unwrap();
        assert_eq!((count, rowid), (1, 1234));
        drop(connection);
        drop(library);
        // Reopening must preserve the migrated key and searchable result.
        let library = Library::open(&database).unwrap();
        assert_eq!(
            library
                .list_assets(&root, &root, &query(Some("replacement")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
    }

    #[test]
    fn search_keys_keep_overlapping_roots_independent_and_clean_up() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let child = root.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("shared.jpg"), b"jpeg").unwrap();
        library.add_root(root.path()).unwrap();
        library.add_root(&child).unwrap();
        library.index_root(root.path()).unwrap();
        library.index_root(&child).unwrap();
        library.remove_root(root.path()).unwrap();
        let child = child.canonicalize().unwrap();
        assert_eq!(
            library
                .list_assets(&child, &child, &query(Some("shared")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
        std::fs::remove_file(child.join("shared.jpg")).unwrap();
        library.index_root(&child).unwrap();
        let connection = library.write();
        for table in ["indexed_asset_search", "indexed_asset_search_keys"] {
            let count: usize = connection
                .query_row(
                    &format!("SELECT COUNT(*) AS count FROM {table}"),
                    [],
                    |row| row.get("count"),
                )
                .unwrap();
            assert_eq!(count, 0, "stale rows in {table}");
        }
    }

    #[test]
    fn batched_reindex_updates_fts_only_to_the_current_asset_set() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        library.add_root(root.path()).unwrap();
        for index in 0..=INDEX_WRITE_BATCH_SIZE {
            std::fs::write(root.path().join(format!("old-{index}.jpg")), b"jpeg").unwrap();
        }
        library.index_root(root.path()).unwrap();

        for index in 0..INDEX_WRITE_BATCH_SIZE {
            std::fs::remove_file(root.path().join(format!("old-{index}.jpg"))).unwrap();
        }
        std::fs::write(root.path().join("new-result.jpg"), b"jpeg").unwrap();
        library.index_root(root.path()).unwrap();

        let canonical_root = root.path().canonicalize().unwrap();
        assert_eq!(
            library
                .list_assets(&canonical_root, &canonical_root, &query(Some("old")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            library
                .list_assets(&canonical_root, &canonical_root, &query(Some("new")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
        let search_rows = library
            .write()
            .query_row(
                "SELECT COUNT(*) AS count FROM indexed_asset_search WHERE root_path = ?1",
                params![canonical_root.to_string_lossy()],
                |row| row.get::<_, usize>("count"),
            )
            .unwrap();
        assert_eq!(search_rows, 2);
    }

    #[test]
    fn reports_index_progress_for_each_directory_batch() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let child = root.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(root.path().join("root.jpg"), b"jpeg").unwrap();
        std::fs::write(child.join("nested.jpg"), b"jpeg").unwrap();
        library.add_root(root.path()).unwrap();
        let mut progress = Vec::new();

        library
            .index_root_with_progress(root.path(), |update| progress.push(update))
            .unwrap();

        let canonical_root = root.path().canonicalize().unwrap();
        assert_eq!(progress.first().unwrap().current_directory, canonical_root);
        assert_eq!(progress.first().unwrap().asset_count, 0);
        assert!(progress.iter().any(|update| {
            update.current_directory == child.canonicalize().unwrap()
                && update.pending_directory_count == 0
        }));
        let completed = progress.last().unwrap();
        assert_eq!(completed.asset_count, 2);
        assert_eq!(completed.directory_count, 1);
        assert_eq!(completed.pending_directory_count, 0);
    }

    #[test]
    fn index_reuses_a_browse_snapshot_created_during_its_generation() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let photo = root.path().join("shared.jpg");
        std::fs::write(&photo, b"jpeg").unwrap();
        library.add_root(root.path()).unwrap();
        let canonical_root = root.path().canonicalize().unwrap();

        library
            .index_root_with_progress_and_directory_scan(
                &canonical_root,
                |_| {},
                |directory| {
                    // Simulate the foreground first page winning while the
                    // index is still discovering directories. Removing the
                    // file afterwards proves the asset phase reused that exact
                    // generation instead of enumerating the directory again.
                    library
                        .browse_directory(&canonical_root, directory, |_| {})
                        .unwrap();
                    std::fs::remove_file(&photo).unwrap();
                    oxy_fs::scan_index_directories(directory)
                },
            )
            .unwrap();

        let page = library
            .list_assets(&canonical_root, &canonical_root, &query(None), 0)
            .unwrap()
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].name, "shared.jpg");
    }

    #[test]
    fn records_the_complete_directory_tree_before_any_assets() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let nested = root.path().join("parent").join("child");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.path().join("root.jpg"), b"jpeg").unwrap();
        std::fs::write(nested.join("nested.jpg"), b"jpeg").unwrap();
        library.add_root(root.path()).unwrap();
        let mut observed_counts = Vec::new();

        library
            .index_root_with_progress(root.path(), |_| {
                let connection = library.write();
                let directory_rows = connection
                    .query_row(
                        "SELECT COUNT(*) AS count FROM indexed_directories",
                        [],
                        |row| row.get::<_, usize>("count"),
                    )
                    .unwrap();
                let asset_rows = connection
                    .query_row("SELECT COUNT(*) AS count FROM indexed_assets", [], |row| {
                        row.get::<_, usize>("count")
                    })
                    .unwrap();
                observed_counts.push((directory_rows, asset_rows));
            })
            .unwrap();

        assert!(observed_counts.contains(&(2, 0)));
        assert!(
            observed_counts
                .iter()
                .filter(|(_, asset_rows)| *asset_rows > 0)
                .all(|(directory_rows, _)| *directory_rows == 2)
        );
    }

    #[test]
    fn directory_search_is_available_before_asset_indexing_finishes() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let matching = root.path().join("matching-folder");
        std::fs::create_dir(&matching).unwrap();
        std::fs::write(root.path().join("root.jpg"), b"jpeg").unwrap();
        library.add_root(root.path()).unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        let mut matches_during_asset_index = None;
        let mut assets_during_asset_index = None;

        library
            .index_root_with_progress(&canonical_root, |progress| {
                if progress.stage == IndexStage::Assets && matches_during_asset_index.is_none() {
                    matches_during_asset_index = Some(
                        library
                            .search_directories(&canonical_root, "matching")
                            .unwrap(),
                    );
                    assets_during_asset_index = Some(
                        library
                            .list_assets(&canonical_root, &canonical_root, &query(None), 0)
                            .unwrap(),
                    );
                }
            })
            .unwrap();

        let matches = matches_during_asset_index.unwrap().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].directory.path, matching.canonicalize().unwrap());
        assert!(matches!(assets_during_asset_index, Some(None)));
    }

    #[test]
    fn index_is_rebuildable_and_removed_with_its_root() {
        let library = Library::in_memory().unwrap();
        let root = tempdir().unwrap();
        let asset = root.path().join("first.jpg");
        std::fs::write(&asset, b"jpeg").unwrap();
        library.add_root(root.path()).unwrap();
        library.index_root(root.path()).unwrap();
        std::fs::remove_file(asset).unwrap();

        library.index_root(root.path()).unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        let page = library
            .list_assets(&canonical_root, &canonical_root, &query(None), 0)
            .unwrap()
            .unwrap();
        assert_eq!(page.total, 0);

        library.remove_root(&canonical_root).unwrap();
        assert!(
            library
                .list_assets(&canonical_root, &canonical_root, &query(None), 0)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn parent_and_child_roots_keep_independent_index_rows() {
        let library = Library::in_memory().unwrap();
        let parent = tempdir().unwrap();
        let child = parent.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("shared.jpg"), b"jpeg").unwrap();
        library.add_root(parent.path()).unwrap();
        library.add_root(&child).unwrap();
        library.index_root(parent.path()).unwrap();
        library.index_root(&child).unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let child = child.canonicalize().unwrap();

        assert_eq!(
            library
                .list_assets(&parent, &parent, &query(Some("shared")), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            library
                .list_assets(&child, &child, &query(None), 0)
                .unwrap()
                .unwrap()
                .total,
            1
        );
    }

    #[test]
    fn scan_generation_survives_database_reopen() {
        let state = tempdir().unwrap();
        let database = state.path().join("library.sqlite");
        let root = tempdir().unwrap();
        let asset = root.path().join("stale.jpg");
        std::fs::write(&asset, b"jpeg").unwrap();
        {
            let library = Library::open(&database).unwrap();
            library.add_root(root.path()).unwrap();
            library.index_root(root.path()).unwrap();
        }
        std::fs::remove_file(asset).unwrap();

        let library = Library::open(&database).unwrap();
        library.index_root(root.path()).unwrap();
        let root = root.path().canonicalize().unwrap();
        let page = library
            .list_assets(&root, &root, &query(None), 0)
            .unwrap()
            .unwrap();
        assert_eq!(page.total, 0);
    }

    #[test]
    fn metadata_projection_survives_database_reopen() {
        let state = tempdir().unwrap();
        let database = state.path().join("library.sqlite");
        let path = state.path().join("rated.HIF");
        let source_revision = "100:200:xmp-a";
        {
            let library = Library::open(&database).unwrap();
            let valid_at = library.next_resource_revision().unwrap();
            let accepted = library
                .accept_metadata_projection(MetadataProjection {
                    path: path.clone(),
                    source_revision: source_revision.into(),
                    state_revision: 0,
                    valid_at,
                    status: ResourceLoadStatus::Ready,
                    rating: Some(4),
                    color_label: Some("Red".into()),
                    pick_label: Some(PickLabel::Accepted),
                    error: None,
                })
                .unwrap();
            assert!(accepted.state_revision > valid_at);
        }

        let reopened = Library::open(&database).unwrap();
        let cached = reopened
            .metadata_projection(&path, source_revision)
            .unwrap()
            .unwrap();
        assert_eq!(cached.rating, Some(4));
        assert_eq!(cached.color_label.as_deref(), Some("Red"));
        assert_eq!(cached.pick_label, Some(PickLabel::Accepted));
    }

    #[test]
    fn cheap_asset_stat_can_publish_a_snapshot_before_full_revision_validation() {
        let library = Library::in_memory().unwrap();
        let path = PathBuf::from("C:/photos/rated.HIF");
        let asset = AssetSummary {
            id: "rated".into(),
            path: path.clone(),
            name: "rated.HIF".into(),
            extension: "HIF".into(),
            kind: AssetKind::Heif,
            size_bytes: 200,
            modified_at_ms: 100,
            has_sidecar: false,
            rating: None,
            color_label: None,
            pick_label: None,
        };
        let valid_at = library.next_resource_revision().unwrap();
        library
            .accept_metadata_projection(MetadataProjection {
                path,
                source_revision: "100:200:0:0:embedded-xmp-digest".into(),
                state_revision: 0,
                valid_at,
                status: ResourceLoadStatus::Ready,
                rating: Some(3),
                color_label: Some("Yellow".into()),
                pick_label: None,
                error: None,
            })
            .unwrap();

        assert_eq!(
            library
                .metadata_projection_for_asset(&asset)
                .unwrap()
                .unwrap()
                .rating,
            Some(3)
        );
        assert!(
            library
                .metadata_projection_for_asset(&AssetSummary {
                    modified_at_ms: 101,
                    ..asset
                })
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sqlite_order_rejects_an_older_result_from_another_connection() {
        let state = tempdir().unwrap();
        let database = state.path().join("library.sqlite");
        let first = Library::open(&database).unwrap();
        let second = Library::open(&database).unwrap();
        let path = state.path().join("shared.HIF");
        let source_revision = "shared-source";
        let older = first.next_resource_revision().unwrap();
        let newer = second.next_resource_revision().unwrap();

        second
            .accept_metadata_projection(MetadataProjection {
                path: path.clone(),
                source_revision: source_revision.into(),
                state_revision: 0,
                valid_at: newer,
                status: ResourceLoadStatus::Ready,
                rating: Some(5),
                color_label: None,
                pick_label: None,
                error: None,
            })
            .unwrap();
        let winner = first
            .accept_metadata_projection(MetadataProjection {
                path: path.clone(),
                source_revision: source_revision.into(),
                state_revision: 0,
                valid_at: older,
                status: ResourceLoadStatus::Ready,
                rating: Some(1),
                color_label: None,
                pick_label: None,
                error: None,
            })
            .unwrap();

        assert_eq!(winner.valid_at, newer);
        assert_eq!(winner.rating, Some(5));
        assert_eq!(
            first
                .metadata_projection(&path, source_revision)
                .unwrap()
                .unwrap()
                .rating,
            Some(5)
        );
    }

    #[test]
    fn stale_descriptor_restore_cannot_overwrite_a_newer_state_of_the_same_request() {
        let library = Library::in_memory().unwrap();
        let loading = library
            .accept_image_projection(ImageProjection {
                path: PathBuf::from("one.HIF"),
                source_revision: "source".into(),
                state_revision: 0,
                valid_at: library.next_resource_revision().unwrap(),
                status: ResourceLoadStatus::Loading,
                level: RenderLevel::Thumbnail,
                result: None,
                error: None,
            })
            .unwrap();
        let terminal = library
            .accept_image_projection(ImageProjection {
                state_revision: 0,
                status: ResourceLoadStatus::Error,
                error: Some("decode failed".into()),
                ..loading.clone()
            })
            .unwrap();
        let restored = library.accept_image_projection(loading).unwrap();
        assert_eq!(restored.state_revision, terminal.state_revision);
        assert_eq!(restored.status, ResourceLoadStatus::Error);
        assert_eq!(restored.error, terminal.error);
    }

    #[test]
    fn image_projection_round_trips_artifact_without_pixel_payload() {
        let library = Library::in_memory().unwrap();
        let directory = tempdir().unwrap();
        let path = directory.path().join("one.HIF");
        let artifact = directory.path().join("one-preview.jpg");
        std::fs::write(&artifact, b"restart-safe artifact").unwrap();
        let valid_at = library.next_resource_revision().unwrap();
        let accepted = library
            .accept_image_projection(ImageProjection {
                path: path.clone(),
                source_revision: "image-source".into(),
                state_revision: 0,
                valid_at,
                status: ResourceLoadStatus::Ready,
                level: RenderLevel::Preview,
                result: Some(oxy_domain::PreviewResult {
                    image_facts: None,
                    geometry: None,
                    path: artifact.clone(),
                    width: 1600,
                    height: 1067,
                    kind: oxy_domain::PreviewKind::Decoded,
                    render_level: RenderLevel::Preview,
                    resource: None,
                    satisfaction: None,
                    persistence: None,
                    diagnostics: None,
                }),
                error: None,
            })
            .unwrap();

        assert!(accepted.state_revision > valid_at);
        let cached = library
            .image_projection(&path, RenderLevel::Preview, "image-source")
            .unwrap()
            .unwrap();
        assert_eq!(cached.result.unwrap().path, artifact);
    }

    #[test]
    fn incompatible_image_projection_can_be_rebuilt_without_losing_ordering() {
        let library = Library::in_memory().unwrap();
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.HIF");
        let artifact = directory.path().join("thumbnail.jpg");
        std::fs::write(&artifact, b"restart-safe artifact").unwrap();
        let valid_at = library.next_resource_revision().unwrap();
        let projection = ImageProjection {
            path: source.clone(),
            source_revision: "source-revision".into(),
            state_revision: 0,
            valid_at,
            status: ResourceLoadStatus::Ready,
            level: RenderLevel::Thumbnail,
            result: Some(oxy_domain::PreviewResult {
                image_facts: None,
                geometry: None,
                path: artifact.clone(),
                width: 160,
                height: 120,
                kind: oxy_domain::PreviewKind::Embedded,
                render_level: RenderLevel::Thumbnail,
                resource: None,
                satisfaction: Some(oxy_domain::MediaSatisfaction::Interim),
                persistence: Some(oxy_domain::MediaPersistence::Persisted),
                diagnostics: None,
            }),
            error: None,
        };
        let mut obsolete = serde_json::to_value(projection.result.as_ref().unwrap()).unwrap();
        // Reproduce the intermediate image-facts schema stored before sampledDimensions.
        obsolete["imageFacts"] = serde_json::json!({
            "exifOrientation": 1,
            "source": {
                "exifOrientation": 1, "revisionId": "source-revision",
                "candidateId": "sony-jpeg", "origin": "embeddedPreview",
                "encodedDimensions": {"width": 160, "height": 120},
                "displayDimensions": {"width": 160, "height": 120}
            },
            "encodedDimensions": {"width": 160, "height": 120},
            "displayDimensions": {"width": 160, "height": 120},
            "detail": {
                "referenceDimensions": {"width": 7008, "height": 4672},
                "region": {"x": 0, "y": 0, "width": 7008, "height": 4672},
                "sampling": "native"
            },
            "processing": [], "byteIntegrity": "sourcePayload"
        });
        assert!(serde_json::from_value::<oxy_domain::PreviewResult>(obsolete.clone()).is_err());
        for incompatible in [obsolete.to_string(), "{".into()] {
            let accepted = library.accept_image_projection(projection.clone()).unwrap();
            library
                .write()
                .execute(
                    "UPDATE resource_projections SET result_json = ?1 WHERE path = ?2",
                    params![incompatible, source.to_string_lossy()],
                )
                .unwrap();
            let cached = library
                .image_projection(&source, RenderLevel::Thumbnail, "source-revision")
                .unwrap()
                .unwrap();
            assert!(cached.result.is_none());
            assert_eq!(cached.valid_at, accepted.valid_at);
            assert_eq!(cached.state_revision, accepted.state_revision);
            let mut stale = projection.clone();
            stale.valid_at = valid_at - 1;
            let rejected = library.accept_image_projection(stale).unwrap();
            assert_eq!(rejected.state_revision, accepted.state_revision);
            assert!(rejected.result.is_none());
            let rebuilt = library.accept_image_projection(projection.clone()).unwrap();
            assert!(rebuilt.state_revision > accepted.state_revision);
            let cached = library
                .image_projection(&source, RenderLevel::Thumbnail, "source-revision")
                .unwrap()
                .unwrap();
            assert_eq!(cached.result.unwrap().path, artifact);
        }
    }

    #[test]
    fn image_projection_strips_process_local_resource_before_sqlite_persistence() {
        let library = Library::in_memory().unwrap();
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.HIF");
        let artifact = directory.path().join("artifact.jpg");
        std::fs::write(&artifact, b"restart-safe artifact").unwrap();
        let valid_at = library.next_resource_revision().unwrap();
        let projection = ImageProjection {
            path: source.clone(),
            source_revision: "source-revision".into(),
            state_revision: 0,
            valid_at,
            status: ResourceLoadStatus::Ready,
            level: RenderLevel::Preview,
            result: Some(oxy_domain::PreviewResult {
                image_facts: None,
                geometry: None,
                path: artifact,
                width: 16,
                height: 8,
                kind: oxy_domain::PreviewKind::Decoded,
                render_level: RenderLevel::Preview,
                resource: Some(oxy_domain::MediaResourceDescriptor {
                    resource_id: "old-process-resource".into(),
                    url: "oxy-media://localhost/resource/old-process-resource".into(),
                    media_type: "image/jpeg".into(),
                }),
                satisfaction: Some(oxy_domain::MediaSatisfaction::Satisfied),
                persistence: Some(oxy_domain::MediaPersistence::Persisted),
                diagnostics: None,
            }),
            error: None,
        };
        let accepted = library.accept_image_projection(projection).unwrap();
        assert!(accepted.result.unwrap().resource.is_some());

        let restored = library
            .image_projection(&source, RenderLevel::Preview, "source-revision")
            .unwrap()
            .unwrap();
        assert!(restored.result.unwrap().resource.is_none());
    }

    #[test]
    fn pending_resource_only_projection_is_not_restart_persisted() {
        let library = Library::in_memory().unwrap();
        let source = PathBuf::from("pending.HIF");
        let valid_at = library.next_resource_revision().unwrap();
        library
            .accept_image_projection(ImageProjection {
                path: source.clone(),
                source_revision: "source-revision".into(),
                state_revision: 0,
                valid_at,
                status: ResourceLoadStatus::Ready,
                level: RenderLevel::Preview,
                result: Some(oxy_domain::PreviewResult {
                    image_facts: None,
                    geometry: None,
                    path: PathBuf::new(),
                    width: 16,
                    height: 8,
                    kind: oxy_domain::PreviewKind::Embedded,
                    render_level: RenderLevel::Preview,
                    resource: Some(oxy_domain::MediaResourceDescriptor {
                        resource_id: "pending-resource".into(),
                        url: "oxy-media://localhost/resource/pending-resource".into(),
                        media_type: "image/jpeg".into(),
                    }),
                    satisfaction: Some(oxy_domain::MediaSatisfaction::Interim),
                    persistence: Some(oxy_domain::MediaPersistence::Pending),
                    diagnostics: None,
                }),
                error: None,
            })
            .unwrap();

        assert!(
            library
                .image_projection(&source, RenderLevel::Preview, "source-revision")
                .unwrap()
                .unwrap()
                .result
                .is_none()
        );
    }

    #[test]
    fn directory_invalidation_blocks_a_late_worker_result() {
        let library = Library::in_memory().unwrap();
        let directory = PathBuf::from("C:/photos");
        let path = directory.join("late.HIF");
        let valid_at = library.next_resource_revision().unwrap();
        let loading = MetadataProjection {
            path: path.clone(),
            source_revision: "source-before-refresh".into(),
            state_revision: 0,
            valid_at,
            status: ResourceLoadStatus::Loading,
            rating: None,
            color_label: None,
            pick_label: None,
            error: None,
        };
        library.accept_metadata_projection(loading.clone()).unwrap();
        library.invalidate_resource_projections(&directory).unwrap();

        let rejected = library
            .accept_metadata_projection(MetadataProjection {
                status: ResourceLoadStatus::Ready,
                rating: Some(5),
                ..loading
            })
            .unwrap();

        assert_eq!(rejected.error.as_deref(), Some("invalidated"));
        assert!(
            library
                .metadata_projection(&path, "source-before-refresh")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sqlite_order_also_rejects_an_older_image_result() {
        let state = tempdir().unwrap();
        let database = state.path().join("library.sqlite");
        let first = Library::open(&database).unwrap();
        let second = Library::open(&database).unwrap();
        let path = state.path().join("shared.HIF");
        let older = first.next_resource_revision().unwrap();
        let newer = second.next_resource_revision().unwrap();
        let old_artifact = state.path().join("old.jpg");
        let new_artifact = state.path().join("new.jpg");
        std::fs::write(&old_artifact, b"old").unwrap();
        std::fs::write(&new_artifact, b"new").unwrap();
        let make_projection = |valid_at, artifact: &Path| ImageProjection {
            path: path.clone(),
            source_revision: "same-image-source".into(),
            state_revision: 0,
            valid_at,
            status: ResourceLoadStatus::Ready,
            level: RenderLevel::Thumbnail,
            result: Some(oxy_domain::PreviewResult {
                image_facts: None,
                geometry: None,
                path: artifact.to_owned(),
                width: 160,
                height: 120,
                kind: oxy_domain::PreviewKind::Embedded,
                render_level: RenderLevel::Thumbnail,
                resource: None,
                satisfaction: None,
                persistence: None,
                diagnostics: None,
            }),
            error: None,
        };

        second
            .accept_image_projection(make_projection(newer, &new_artifact))
            .unwrap();
        let winner = first
            .accept_image_projection(make_projection(older, &old_artifact))
            .unwrap();

        assert_eq!(winner.valid_at, newer);
        assert_eq!(winner.result.unwrap().path, new_artifact);
    }
}
