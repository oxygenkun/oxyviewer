pub(crate) mod cache;
pub(crate) mod external_apps;

use crate::{jobs, providers};
use oxy_fs::FsCatalog;
use oxy_library::Library;
use oxy_runtime::JobRegistry;
use std::sync::Arc;

pub(crate) struct AppState {
    pub(crate) external_apps: Arc<external_apps::ExternalAppManager>,
    pub(crate) files: Arc<FsCatalog>,
    pub(crate) jobs: JobRegistry,
    pub(crate) library: Arc<Library>,
    pub(crate) cache: Arc<cache::CacheManager>,
    pub(crate) heif: Arc<oxy_media::HeifDecodeService>,
    pub(crate) media_resources: oxy_media::ResourceRegistry,
    pub(crate) metadata: oxy_metadata::MetadataFacade,
    pub(crate) metadata_queue: jobs::metadata::MetadataQueue,
    pub(crate) preview_queue: jobs::preview::PreviewQueue,
    pub(crate) debug_snapshots: Arc<jobs::DebugSnapshotCache>,
    pub(crate) directory_tree_queue: jobs::directory_tree::DirectoryTreeQueue,
    pub(crate) library_index_queue: jobs::LibraryIndexQueue,
    pub(crate) metadata_provider: Arc<providers::exiftool::ProviderManager>,
    /// Caches burst values so growing listings are grouped without re-reading files.
    pub(crate) burst_scan: Arc<oxy_metadata::BurstScanner>,
}
