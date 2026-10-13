//! Application assembly and lifetime, shared by desktop and headless hosts.
mod external_apps;
pub use external_apps::ExternalAppManager;

use crate::{
    events::CoreEvent,
    files::FsCatalog,
    jobs::JobRegistry,
    library::Library,
    metadata::{BurstScanner, MetadataFacade, provider::ProviderManager},
    people::{People, execution::operations::PersonOperations},
    resources::{
        DebugSnapshotCache, LibraryIndexQueue, cache::CacheManager,
        directory_tree::DirectoryTreeQueue, metadata::MetadataQueue, preview::PreviewQueue,
    },
    runtime::CoreRuntime,
    storage::Store,
    tags::Tags,
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

/// Every handle belongs to this application and shares one store and lifetime.
pub struct OxyApp {
    pub runtime: CoreRuntime,
    pub external_apps: Arc<ExternalAppManager>,
    pub files: Arc<FsCatalog>,
    pub jobs: JobRegistry,
    pub library: Arc<Library>,
    pub tags: Arc<Tags>,
    pub people: Arc<People>,
    pub person_operations: Arc<PersonOperations>,
    pub person_model_dir: PathBuf,
    pub cache: Arc<CacheManager>,
    pub heif: Arc<oxy_media::HeifDecodeService>,
    pub media_resources: oxy_media::ResourceRegistry,
    pub metadata: MetadataFacade,
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
        let library = Arc::new(Library::with_store(store.clone()));
        let tags = Arc::new(Tags::new(store.clone()));
        let people = Arc::new(People::new(store));
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
        let stopping_library = library.clone();
        let stopping_people = person_operations.clone();
        runtime.on_stop(move || {
            stopping_library.stop_background();
            stopping_people.cancel_current();
        });
        let directory_tree_queue =
            DirectoryTreeQueue::new(runtime.clone(), files.clone(), library.clone());
        let metadata_queue = MetadataQueue::new(
            runtime.clone(),
            files.clone(),
            metadata.clone(),
            library.clone(),
            tags.clone(),
        );
        let preview_queue = PreviewQueue::new(runtime.clone(), library.clone(), cache.clone());
        let library_index_queue = LibraryIndexQueue::new(library.clone(), files.clone());
        cache.schedule_prune(None);
        Ok((
            Self {
                runtime,
                external_apps,
                files,
                jobs: JobRegistry::default(),
                library,
                tags,
                people,
                person_operations,
                person_model_dir,
                cache,
                heif: Arc::new(oxy_media::HeifDecodeService::default()),
                media_resources: oxy_media::shared_resource_registry(),
                metadata,
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
        self.library
            .shutdown_background(deadline.saturating_duration_since(std::time::Instant::now()))
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
                    crate::contracts::MetadataRequestPriority::Selected
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
            .reserve(crate::contracts::PersonOperationStatus {
                operation_id: "owned-worker".into(),
                folder_path: None,
                state: crate::contracts::PersonOperationState::Preparing,
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
            crate::contracts::PersonOperationState::Cancelled
        );
    }
}
