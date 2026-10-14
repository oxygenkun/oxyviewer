//! Application assembly and lifetime, shared by desktop and headless hosts.

mod external_apps;
pub mod types;
pub(crate) mod workflows;
pub use external_apps::ExternalAppManager;

use crate::{
    browse::BrowseService,
    events::CoreEvent,
    files::{FileService, FsCatalog},
    jobs::JobRegistry,
    library::{Library, LibraryService},
    metadata::{BurstScanner, MetadataFacade, MetadataService, provider::ProviderManager},
    people::{People, PersonService, execution::operations::PersonOperations},
    resources::{
        DebugSnapshotCache, LibraryIndexQueue, cache::CacheManager,
        directory_tree::DirectoryTreeQueue, metadata::MetadataQueue, preview::PreviewQueue,
    },
    runtime::CoreRuntime,
    storage::Store,
    tags::{Tags, xmp_sync::TagXmpSync},
};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc::Receiver},
    time::Duration,
};

pub struct AppPaths {
    pub data_dir: PathBuf,
    pub preview_dir: PathBuf,
}

impl AppPaths {
    pub fn from_roots(data_root: PathBuf, cache_root: PathBuf) -> Self {
        Self {
            data_dir: data_root.join(crate::settings::DATA_NAMESPACE),
            preview_dir: cache_root
                .join(crate::settings::DATA_NAMESPACE)
                .join("previews"),
        }
    }
}

/// Every handle belongs to this application and shares one store and lifetime.
pub struct OxyApp {
    pub runtime: CoreRuntime,
    pub external_apps: Arc<ExternalAppManager>,
    pub files: Arc<FsCatalog>,
    pub file_service: Arc<FileService>,
    pub jobs: Arc<JobRegistry>,
    pub library: Arc<Library>,
    pub library_service: Arc<LibraryService>,
    pub tags: Arc<Tags>,
    pub tag_sync: Arc<TagXmpSync>,
    pub browse: Arc<BrowseService>,
    pub people: Arc<People>,
    pub person_service: Arc<PersonService>,
    pub person_operations: Arc<PersonOperations>,
    pub person_model_dir: PathBuf,
    pub cache: Arc<CacheManager>,
    pub heif: Arc<oxy_media::HeifDecodeService>,
    pub media_resources: oxy_media::ResourceRegistry,
    pub metadata: MetadataFacade,
    pub metadata_service: Arc<MetadataService>,
    pub metadata_queue: MetadataQueue,
    pub preview_queue: PreviewQueue,
    pub debug_snapshots: Arc<DebugSnapshotCache>,
    pub directory_tree_queue: DirectoryTreeQueue,
    pub library_index_queue: LibraryIndexQueue,
    pub metadata_provider: Arc<ProviderManager>,
    pub burst_scan: Arc<BurstScanner>,
}

impl OxyApp {
    pub fn open(paths: AppPaths) -> Result<(Self, Receiver<CoreEvent>), String> {
        std::fs::create_dir_all(&paths.data_dir).map_err(|error| error.to_string())?;
        let (runtime, events) = CoreRuntime::channel();
        let store = Arc::new(
            Store::open(&paths.data_dir.join("oxyviewer.sqlite"))
                .map_err(|error| error.to_string())?,
        );
        let foreground = Arc::new(crate::jobs::ForegroundGate::default());
        let snapshots = Arc::new(crate::browse::snapshots::BrowseSnapshots::new(
            store.clone(),
            foreground.clone(),
        ));
        let library = Arc::new(Library::with_store(
            store.clone(),
            snapshots.clone(),
            foreground,
        ));
        let projections = Arc::new(crate::resources::projections::ResourceProjections::new(
            store.clone(),
        ));
        let tags = Arc::new(Tags::new(store.clone()));
        let people = Arc::new(People::new(store.clone()));
        let person_model_dir = paths.data_dir.join("person-models");
        let external_apps = Arc::new(ExternalAppManager::load(
            paths.data_dir.join("external-apps.json"),
        ));
        let metadata_provider = Arc::new(ProviderManager::load(paths.data_dir.clone()));
        let metadata = metadata_provider.facade();
        let cache = Arc::new(CacheManager::load(
            runtime.clone(),
            paths.preview_dir,
            paths.data_dir.join("cache-settings.json"),
        )?);
        let files = Arc::new(FsCatalog::default());
        let person_operations = Arc::new(PersonOperations::default());
        let person_service = Arc::new(PersonService {
            people: people.clone(),
            files: files.clone(),
            person_operations: person_operations.clone(),
            runtime: runtime.clone(),
            person_model_dir: person_model_dir.clone(),
            cache: cache.clone(),
        });
        let stopping_foreground = library.foreground.clone();
        let stopping_snapshots = snapshots.clone();
        let stopping_people = person_operations.clone();
        runtime.on_stop(move || {
            stopping_foreground.stop();
            stopping_snapshots.stop();
            stopping_people.cancel_current();
        });
        let directory_tree_queue =
            DirectoryTreeQueue::new(runtime.clone(), files.clone(), library.clone());
        let metadata_queue = MetadataQueue::new(
            runtime.clone(),
            files.clone(),
            metadata.clone(),
            projections.clone(),
            tags.clone(),
        );
        let preview_queue = PreviewQueue::new(
            runtime.clone(),
            library.clone(),
            projections.clone(),
            cache.clone(),
        );
        let jobs = Arc::new(JobRegistry::default());
        let metadata_service = Arc::new(MetadataService {
            runtime: runtime.clone(),
            files: files.clone(),
            metadata: metadata.clone(),
            queue: metadata_queue.clone(),
            jobs: jobs.clone(),
        });
        let library_index_queue = LibraryIndexQueue::new(library.clone(), files.clone());
        let library_service = Arc::new(LibraryService {
            snapshots: snapshots.clone(),
            library: library.clone(),
            cache: cache.clone(),
            index: library_index_queue.clone(),
            runtime: runtime.clone(),
        });
        let tag_sync = Arc::new(TagXmpSync::new(
            runtime.clone(),
            tags.clone(),
            files.clone(),
            metadata.clone(),
        ));
        let file_service = Arc::new(FileService {
            store,
            snapshots: snapshots.clone(),
            files: files.clone(),
            metadata: metadata_queue.clone(),
            preview: preview_queue.clone(),
            tag_sync: tag_sync.clone(),
            runtime: runtime.clone(),
            gate: Default::default(),
        });
        let browse = Arc::new(BrowseService {
            snapshots,
            projections,
            files: files.clone(),
            library: library.clone(),
            metadata: metadata.clone(),
            tags: tags.clone(),
            people: people.clone(),
            metadata_queue: metadata_queue.clone(),
            preview_queue: preview_queue.clone(),
            directory_tree_queue: directory_tree_queue.clone(),
            library_index_queue: library_index_queue.clone(),
            runtime: runtime.clone(),
            tag_sync: tag_sync.clone(),
        });
        cache.schedule_prune(None);
        Ok((
            Self {
                runtime,
                external_apps,
                files,
                file_service,
                jobs,
                library,
                library_service,
                tags,
                tag_sync,
                browse,
                people,
                person_service,
                person_operations,
                person_model_dir,
                cache,
                heif: Arc::new(oxy_media::HeifDecodeService::default()),
                media_resources: oxy_media::shared_resource_registry(),
                metadata,
                metadata_service,
                metadata_queue,
                preview_queue,
                debug_snapshots: Default::default(),
                directory_tree_queue,
                library_index_queue,
                metadata_provider,
                burst_scan: Default::default(),
            },
            events,
        ))
    }

    /// Native calls already in progress must unwind cooperatively; the deadline
    /// reports that condition without pretending those threads were interrupted.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        self.jobs.cancel_all();
        if !self.runtime.shutdown(timeout) {
            return false;
        }
        self.browse
            .snapshots
            .shutdown(deadline.saturating_duration_since(std::time::Instant::now()))
    }
}

impl Drop for OxyApp {
    fn drop(&mut self) {
        self.jobs.cancel_all();
        self.runtime.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_bootstrap_leaves_old_data_and_previews_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        let cache = directory.path().join("cache");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(cache.join("previews")).unwrap();
        let old_db = data.join("oxyviewer.sqlite");
        std::fs::write(&old_db, b"old application data").unwrap();
        let old_preview = cache.join("previews/old.jpg");
        std::fs::write(&old_preview, b"old preview").unwrap();
        let paths = AppPaths::from_roots(data, cache);
        let current_db = paths.data_dir.join("oxyviewer.sqlite");
        let (app, _) = OxyApp::open(paths).unwrap();
        assert!(current_db.exists());
        assert!(
            app.cache
                .preview_dir()
                .components()
                .any(|part| part.as_os_str() == crate::settings::DATA_NAMESPACE)
        );
        assert!(app.shutdown(Duration::from_secs(2)));
        assert_eq!(std::fs::read(old_db).unwrap(), b"old application data");
        assert_eq!(std::fs::read(old_preview).unwrap(), b"old preview");
    }

    #[test]
    fn headless_host_assembles_and_stops_all_queue_workers() {
        let directory = tempfile::tempdir().unwrap();
        let (app, events) = OxyApp::open(AppPaths {
            data_dir: directory.path().join("data"),
            preview_dir: directory.path().join("previews"),
        })
        .unwrap();
        assert!(app.runtime.active_workers() >= 4);
        assert!(app.shutdown(Duration::from_secs(2)));
        assert_eq!(app.runtime.active_workers(), 0);
        assert!(matches!(events.recv().unwrap(), CoreEvent::Shutdown));
        assert!(
            app.metadata_queue
                .request_summaries(
                    &app.runtime,
                    vec![],
                    crate::resources::types::MetadataRequestPriority::Selected
                )
                .is_err()
        );
    }

    #[test]
    fn shutdown_cancels_owned_person_work_and_waits_for_its_exit() {
        let directory = tempfile::tempdir().unwrap();
        let (app, _) = OxyApp::open(AppPaths {
            data_dir: directory.path().join("data"),
            preview_dir: directory.path().join("previews"),
        })
        .unwrap();
        let operation = app
            .person_operations
            .reserve(crate::people::types::PersonOperationStatus {
                operation_id: "owned-worker".into(),
                folder_path: None,
                state: crate::people::types::PersonOperationState::Preparing,
                completed: 0,
                total: 0,
                detail: String::new(),
                error: None,
                run: None,
            })
            .unwrap();
        let work = operation.clone();
        let runtime = app.runtime.clone();
        PersonOperations::launch(&app.runtime, operation.clone(), move || {
            runtime.wait_for_stop(Duration::from_secs(30));
            assert!(work.is_cancelled());
            Ok(())
        })
        .unwrap();
        assert!(app.shutdown(Duration::from_secs(2)));
        assert_eq!(
            operation.snapshot().unwrap().state,
            crate::people::types::PersonOperationState::Cancelled
        );
    }
}
