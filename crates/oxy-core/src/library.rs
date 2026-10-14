//! Registered library roots and their derived index.
//! User facts survive cache clearing. The feature owns its declarations and
//! statements; shared SQLite mechanisms live in storage.

pub(crate) mod repo;
pub(crate) mod schema;
mod service;
pub mod types;
pub use service::LibraryService;

mod cache;
pub(crate) use cache::index::forget_root;
pub use cache::index::{IndexProgress, IndexStage, IndexStats};
#[cfg(test)]
mod audit;
mod user;

use crate::storage::Store;
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
    #[error(transparent)]
    Store(#[from] crate::storage::StoreError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Filesystem(#[from] crate::files::FsError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("folder order must contain every library root exactly once")]
    InvalidRootOrder,
    #[error("cannot relocate folder: {0}")]
    InvalidRelocation(String),
}

/// Registered roots and their index over the application store. OxyApp supplies
/// the browse snapshot handle and shared foreground gate, so indexing can reuse
/// recently validated summaries without owning snapshot policy or persistence.
pub struct Library {
    store: Arc<Store>,
    pub(crate) snapshots: Arc<crate::browse::snapshots::BrowseSnapshots>,
    pub foreground: Arc<crate::jobs::ForegroundGate>,
    indexing_roots: Mutex<HashSet<PathBuf>>,
    pub(crate) index_gate: Mutex<()>,
}

impl Library {
    /// The write connection. Domain modules hold it for the duration of a write.
    pub(crate) fn write(&self) -> MutexGuard<'_, Connection> {
        self.store.write()
    }

    /// A read connection that never waits for a background index write.
    pub(crate) fn read_connection(&self) -> MutexGuard<'_, Connection> {
        self.store.read()
    }

    /// Opens the library over the store at `path`.
    ///
    /// The store creates the file, enables WAL, registers the vector extension,
    /// and brings the schema up to date with user-owned tables first. Nothing
    /// about the schema is decided here, so a storage version bump cannot be
    /// something this constructor forgets to run.
    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        Ok(Self::from_store(Arc::new(Store::open(path)?)))
    }

    /// Opens a library that lives only in this process.
    pub fn in_memory() -> Result<Self, LibraryError> {
        Ok(Self::from_store(Arc::new(Store::in_memory()?)))
    }

    /// Standalone harnesses share an isolated snapshot handle and foreground
    /// gate over this store. OxyApp supplies its own assembled handles.
    fn from_store(store: Arc<Store>) -> Self {
        let foreground = Arc::new(crate::jobs::ForegroundGate::default());
        let snapshots = Arc::new(crate::browse::snapshots::BrowseSnapshots::new(
            store.clone(),
            foreground.clone(),
        ));
        Self::with_store(store, snapshots, foreground)
    }

    pub(crate) fn with_store(
        store: Arc<Store>,
        snapshots: Arc<crate::browse::snapshots::BrowseSnapshots>,
        foreground: Arc<crate::jobs::ForegroundGate>,
    ) -> Self {
        Self {
            store,
            snapshots,
            indexing_roots: Mutex::new(HashSet::new()),
            index_gate: Mutex::new(()),
            foreground,
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
    use crate::browse::types::{AssetQuery, AssetSort, SortDirection};
    use crate::resources::types::{MetadataProjection, ResourceLoadStatus};
    use oxy_media::types::RenderLevel;
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
                    crate::tags::repo::list_tags(&library.store().read())
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
        let projections = crate::resources::projections::ResourceProjections::new(library.store());
        let path = state.path().join("photo.jpg");
        let valid_at = projections.next_resource_revision().unwrap();
        projections
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
                    projections
                        .metadata_projection(&path, "100:200:xmp-a")
                        .unwrap()
                        .unwrap()
                        .rating,
                    Some(4)
                );
                assert!(
                    projections
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
        let projections = crate::resources::projections::ResourceProjections::new(library.store());
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::write(root.join("visible.jpg"), b"jpeg").unwrap();
        library.add_root(&root).unwrap();
        library.index_root(&root).unwrap();
        let template = crate::files::scan_index_assets(&root).unwrap().remove(0);
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
                projections
                    .metadata_projection(&template.path, "missing")
                    .unwrap();
                projections
                    .image_projection(&template.path, RenderLevel::Thumbnail, "missing")
                    .unwrap();
                reads.push(started.elapsed());
                let started = Instant::now();
                projections.next_resource_revision().unwrap();
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
        let template = crate::files::scan_index_assets(&root).unwrap().remove(0);
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
    fn replaces_changed_search_terms_without_duplicates() {
        let state = tempdir().unwrap();
        let database = state.path().join("library.sqlite");
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::write(root.join("photo.jpg"), b"jpeg").unwrap();
        let library = Library::open(&database).unwrap();
        library.add_root(&root).unwrap();
        library.index_root(&root).unwrap();
        // Exercise a valid current-format index with a non-default search rowid.
        library
            .write()
            .execute_batch(
                "UPDATE indexed_asset_search SET rowid = 1234;
             UPDATE indexed_asset_search_keys SET search_rowid=1234;",
            )
            .unwrap();
        drop(library);

        let library = Library::open(&database).unwrap();
        let mut assets = crate::files::scan_index_assets(&root).unwrap();
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
        // Reopening must preserve the current key and searchable result.
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
                        .snapshots
                        .browse_directory(&canonical_root, directory, |_| {})
                        .unwrap();
                    std::fs::remove_file(&photo).unwrap();
                    crate::files::scan_index_directories(directory)
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
}
