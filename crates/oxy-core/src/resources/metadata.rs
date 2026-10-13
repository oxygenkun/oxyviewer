use crate::browse::types::AssetSummary;
use crate::files::FsCatalog;
use crate::jobs::CoalescingPriorityQueue;
use crate::library::Library;
use crate::metadata::types::{
    AssetDetails, AssetDetailsResult, MetadataCapability, MetadataProvider,
};
use crate::metadata::{
    MetadataDocument, MetadataFacade, MetadataObservation, metadata_source_revision,
};
use crate::resources::types::{
    DebugQueueItem, DebugQueueState, MetadataProjection, MetadataRequestPriority,
    ResourceLoadStatus,
};
use crate::tags::Tags;
use crate::{events::CoreEvent, runtime::CoreRuntime};
use oxy_media::types::{AssetKind, EditableMetadata};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
};

pub const METADATA_PROJECTION_UPDATED_EVENT: &str = "metadata-projection-updated";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RequestKey {
    path: PathBuf,
    source_revision: String,
    generation: u64,
}

struct Request {
    asset: AssetSummary,
    observation: MetadataObservation,
    detail_waiters: Vec<Sender<AssetDetailsResult>>,
    priority: MetadataRequestPriority,
}

#[derive(Default)]
struct WorkState {
    stopping: bool,
    admission_locks: HashMap<RequestKey, Arc<Mutex<()>>>,
    pending: CoalescingPriorityQueue<RequestKey, Request, MetadataRequestPriority>,
    active: HashMap<RequestKey, Arc<Mutex<Request>>>,
    active_directory: Option<PathBuf>,
}

#[derive(Clone)]
pub struct MetadataQueue {
    files: Arc<FsCatalog>,
    metadata: MetadataFacade,
    library: Arc<Library>,
    /// A sidecar's keywords are the tag vocabulary's business, not the
    /// library's.
    tags: Arc<Tags>,
    work: Arc<(Mutex<WorkState>, Condvar)>,
    details: Arc<RwLock<HashMap<RequestKey, AssetDetailsResult>>>,
    request_generation: Arc<AtomicU64>,
}

impl MetadataQueue {
    pub fn new(
        app: CoreRuntime,
        files: Arc<FsCatalog>,
        metadata: MetadataFacade,
        library: Arc<Library>,
        tags: Arc<Tags>,
    ) -> Self {
        let queue = Self {
            files,
            metadata,
            library,
            tags,
            work: Arc::new((Mutex::new(WorkState::default()), Condvar::new())),
            details: Arc::new(RwLock::new(HashMap::new())),
            request_generation: Arc::new(AtomicU64::new(0)),
        };
        let stop_work = Arc::clone(&queue.work);
        app.on_stop(move || {
            let mut work = stop_work
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            work.stopping = true;
            work.pending.remove_if(|_, _| true);
            for request in work.active.values() {
                request
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .detail_waiters
                    .clear();
            }
            stop_work.1.notify_all();
        });
        queue.spawn_worker(app);
        queue
    }

    pub fn request_summaries(
        &self,
        app: &CoreRuntime,
        paths: Vec<PathBuf>,
        priority: MetadataRequestPriority,
    ) -> Result<Vec<MetadataProjection>, String> {
        if app.is_stopping() {
            return Err("application is shutting down".into());
        }
        let mut snapshots = Vec::with_capacity(paths.len());
        for path in paths {
            let asset = self
                .files
                .get_asset(&path)
                .map_err(|error| error.to_string())?;
            let optimistic = self
                .library
                .metadata_projection_for_asset(&asset)
                .map_err(|error| error.to_string())?;
            if let Some(projection) = &optimistic {
                let _ = app.publish(CoreEvent::MetadataProjectionUpdated(projection.clone()));
            }
            let source_revision = metadata_source_revision(&asset);
            if let Some(cached) = self
                .library
                .metadata_projection(&asset.path, &source_revision)
                .map_err(|error| error.to_string())?
                && cached.status == ResourceLoadStatus::Ready
            {
                if optimistic.as_ref().map(|value| value.state_revision)
                    != Some(cached.state_revision)
                {
                    let _ = app.publish(CoreEvent::MetadataProjectionUpdated(cached.clone()));
                }
                snapshots.push(cached);
                continue;
            }
            snapshots.push(self.enqueue(app, asset, source_revision, priority, None)?);
        }
        Ok(snapshots)
    }

    pub fn invalidate_directory(&self, directory: &std::path::Path) {
        self.request_generation.fetch_add(1, Ordering::Relaxed);
        self.metadata.invalidate_summary_directory(directory);
        self.details
            .write()
            .expect("metadata details lock poisoned")
            .retain(|key, _| key.path.parent() != Some(directory));
    }

    /// Drops work that has not started for assets outside the directory the
    /// user is currently viewing. Active reads are allowed to finish so cache
    /// and projection transitions remain consistent.
    pub fn clear_pending_outside_directory(&self, directory: &std::path::Path) -> usize {
        let mut work = self.work.0.lock().expect("metadata queue lock poisoned");
        work.active_directory = Some(directory.to_owned());
        work.pending
            .remove_if(|key, _| key.path.parent() != Some(directory))
            .len()
    }

    pub fn debug_snapshot(&self) -> Option<DebugQueueState> {
        let work = self.work.0.try_lock().ok()?;
        let pending = work
            .pending
            .entries()
            .map(|(key, request, priority)| metadata_debug_item(key, request, priority))
            .collect::<Vec<_>>();
        let active = work
            .active
            .iter()
            .map(|(key, request)| {
                let request = request.try_lock().ok()?;
                Some(metadata_debug_item(key, &request, request.priority))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(DebugQueueState {
            name: "metadata".into(),
            concurrency: 1,
            pending,
            active,
        })
    }

    pub fn request_details(
        &self,
        app: &CoreRuntime,
        path: PathBuf,
    ) -> Result<Receiver<AssetDetailsResult>, String> {
        if app.is_stopping() {
            return Err("application is shutting down".into());
        }
        let asset = self
            .files
            .get_asset(&path)
            .map_err(|error| error.to_string())?;
        let source_revision = metadata_source_revision(&asset);
        let key = RequestKey {
            path: asset.path.clone(),
            source_revision: source_revision.clone(),
            generation: self.request_generation.load(Ordering::Relaxed),
        };
        let (sender, receiver) = mpsc::channel();
        if let Some(cached) = self
            .details
            .read()
            .expect("metadata details lock poisoned")
            .get(&key)
            .cloned()
        {
            let _ = sender.send(cached);
            return Ok(receiver);
        }
        self.enqueue(
            app,
            asset,
            source_revision,
            MetadataRequestPriority::Selected,
            Some(sender),
        )?;
        Ok(receiver)
    }

    fn enqueue(
        &self,
        app: &CoreRuntime,
        asset: AssetSummary,
        source_revision: String,
        priority: MetadataRequestPriority,
        detail_waiter: Option<Sender<AssetDetailsResult>>,
    ) -> Result<MetadataProjection, String> {
        let key = RequestKey {
            path: asset.path.clone(),
            source_revision,
            generation: self.request_generation.load(Ordering::Relaxed),
        };
        let gate = {
            let mut work = self.work.0.lock().expect("metadata queue lock poisoned");
            if work.stopping {
                return Err("application is shutting down".into());
            }
            Arc::clone(work.admission_locks.entry(key.clone()).or_default())
        };
        let admission = gate.lock().expect("metadata admission lock poisoned");
        let result = self.enqueue_admitted(app, asset, key.clone(), priority, detail_waiter);
        drop(admission);
        let mut work = self.work.0.lock().expect("metadata queue lock poisoned");
        if Arc::strong_count(&gate) == 2 {
            work.admission_locks.remove(&key);
        }
        result
    }

    fn enqueue_admitted(
        &self,
        app: &CoreRuntime,
        asset: AssetSummary,
        key: RequestKey,
        priority: MetadataRequestPriority,
        detail_waiter: Option<Sender<AssetDetailsResult>>,
    ) -> Result<MetadataProjection, String> {
        let mut work = self.work.0.lock().expect("metadata queue lock poisoned");
        if work.stopping {
            return Err("application is shutting down".into());
        }
        if work
            .active_directory
            .as_deref()
            .is_some_and(|directory| key.path.parent() != Some(directory))
        {
            return Err("metadata request left the active directory".into());
        }
        if let Some(active) = work.active.get(&key) {
            if let Some(waiter) = detail_waiter {
                let mut active = active
                    .lock()
                    .expect("active metadata request lock poisoned");
                active.detail_waiters.push(waiter);
                active.priority = active.priority.max(priority);
            }
            return self
                .metadata
                .cached_projection(&asset)
                .ok_or_else(|| "active metadata request has no projection".to_owned());
        }
        if work.pending.contains_key(&key) {
            let waiters = detail_waiter.into_iter().collect::<Vec<_>>();
            let updated = work.pending.update_if_present(&key, priority, |current| {
                current.detail_waiters.extend(waiters);
            });
            debug_assert!(updated);
            return self
                .metadata
                .cached_projection(&asset)
                .ok_or_else(|| "pending metadata request has no projection".to_owned());
        }
        drop(work);

        let valid_at = self
            .library
            .next_resource_revision()
            .map_err(|error| error.to_string())?;
        let observation = self.metadata.observe_asset_at(&asset, valid_at);
        if let Some(cached) = self
            .library
            .metadata_projection(&asset.path, &key.source_revision)
            .map_err(|error| error.to_string())?
        {
            self.metadata.restore_projection(&observation, &cached);
        } else if let Some(cached) = self
            .library
            .metadata_projection_for_asset(&asset)
            .map_err(|error| error.to_string())?
        {
            self.metadata.restore_projection(&observation, &cached);
        }
        let loading = self
            .library
            .accept_metadata_projection(self.metadata.mark_observation_loading(&observation))
            .map_err(|error| error.to_string())?;
        let request = Request {
            asset,
            observation,
            detail_waiters: detail_waiter.into_iter().collect(),
            priority,
        };
        let mut work = self.work.0.lock().expect("metadata queue lock poisoned");
        if work.stopping
            || key.generation != self.request_generation.load(Ordering::Relaxed)
            || work
                .active_directory
                .as_deref()
                .is_some_and(|directory| key.path.parent() != Some(directory))
        {
            return Err("metadata request invalidated during admission".into());
        }
        work.pending
            .push_or_merge(key, request, priority, |_, _| unreachable!());
        drop(work);
        self.work.1.notify_one();
        let _ = app.publish(CoreEvent::MetadataProjectionUpdated(loading.clone()));
        Ok(loading)
    }

    fn spawn_worker(&self, app: CoreRuntime) {
        let queue = self.clone();
        app.clone()
            .spawn_named("oxy-metadata-projection".into(), move || {
                loop {
                    let request = {
                        let mut pending =
                            queue.work.0.lock().expect("metadata queue lock poisoned");
                        while pending.pending.is_empty() && !app.is_stopping() {
                            pending = queue
                                .work
                                .1
                                .wait(pending)
                                .expect("metadata queue lock poisoned");
                        }
                        if app.is_stopping() {
                            return;
                        }
                        pending.pending.pop().map(|(key, mut request, priority)| {
                            request.priority = priority;
                            let request = Arc::new(Mutex::new(request));
                            pending.active.insert(key.clone(), request.clone());
                            (key, request)
                        })
                    };
                    let Some((key, request)) = request else {
                        continue;
                    };
                    let (asset, observation, needs_details) = {
                        let request = request
                            .lock()
                            .expect("active metadata request lock poisoned");
                        (
                            request.asset.clone(),
                            request.observation.clone(),
                            !request.detail_waiters.is_empty(),
                        )
                    };
                    if !needs_details {
                        let candidate = match queue.metadata.read_summary_observation(&observation)
                        {
                            Ok(projection) => projection,
                            Err(error) => queue
                                .metadata
                                .fail_observation(&observation, error.to_string()),
                        };
                        if app.is_stopping() {
                            return;
                        }
                        let projection = match queue.library.accept_metadata_projection(candidate) {
                            Ok(projection) => projection,
                            Err(error) => {
                                eprintln!("failed to persist metadata projection: {error}");
                                queue
                                    .metadata
                                    .fail_observation(&observation, error.to_string())
                            }
                        };
                        if projection.error.as_deref() != Some("invalidated") {
                            let _ = app.publish(CoreEvent::MetadataProjectionUpdated(projection));
                        }
                        let should_upgrade = {
                            let mut work =
                                queue.work.0.lock().expect("metadata queue lock poisoned");
                            let should_upgrade = !request
                                .lock()
                                .expect("active metadata request lock poisoned")
                                .detail_waiters
                                .is_empty();
                            if !should_upgrade {
                                work.active.remove(&key);
                            }
                            should_upgrade
                        };
                        if !should_upgrade {
                            continue;
                        }
                    }

                    let dimensions = oxy_media::dimensions(&asset.path, asset.kind).ok();
                    let display_dimensions = dimensions.map(|value| (value.width, value.height));
                    let document = queue
                        .metadata
                        .read_document_observation(&observation, display_dimensions);
                    if app.is_stopping() {
                        return;
                    }
                    if asset.has_sidecar
                        && let Ok((document, _)) = &document
                        && let Err(error) = queue.tags.import_sidecar_tags(
                            &asset.path,
                            &document.editable.keywords,
                            &document.editable.hierarchical_keywords,
                        )
                    {
                        eprintln!("failed to import hierarchical tags: {error}");
                    }
                    let (document, candidate) = match document {
                        Ok((document, projection)) => (Ok(document), projection),
                        Err(error) => {
                            let projection = queue
                                .metadata
                                .fail_observation(&observation, error.to_string());
                            (Err(error), projection)
                        }
                    };
                    let projection = match queue.library.accept_metadata_projection(candidate) {
                        Ok(projection) => projection,
                        Err(error) => {
                            eprintln!("failed to persist metadata projection: {error}");
                            queue
                                .metadata
                                .fail_observation(&observation, error.to_string())
                        }
                    };
                    let details = build_asset_details(asset, dimensions, document);
                    let result = AssetDetailsResult {
                        details,
                        metadata_projection: projection.clone(),
                    };
                    queue
                        .details
                        .write()
                        .expect("metadata details lock poisoned")
                        .insert(key.clone(), result.clone());
                    if projection.error.as_deref() != Some("invalidated") {
                        let _ = app.publish(CoreEvent::MetadataProjectionUpdated(projection));
                    }
                    let waiters = {
                        let mut work = queue.work.0.lock().expect("metadata queue lock poisoned");
                        let waiters = std::mem::take(
                            &mut request
                                .lock()
                                .expect("active metadata request lock poisoned")
                                .detail_waiters,
                        );
                        work.active.remove(&key);
                        waiters
                    };
                    for waiter in waiters {
                        let _ = waiter.send(result.clone());
                    }
                }
            })
            .expect("failed to start metadata projection worker");
    }
}

fn metadata_debug_item(
    key: &RequestKey,
    request: &Request,
    priority: MetadataRequestPriority,
) -> DebugQueueItem {
    DebugQueueItem {
        key: format!("{}:{}", key.path.display(), key.generation),
        path: Some(key.path.clone()),
        root_path: None,
        stage: if request.detail_waiters.is_empty() {
            "summary".into()
        } else {
            "details".into()
        },
        priority: format!("{priority:?}").to_lowercase(),
        rank: None,
        consumers: request.detail_waiters.len().max(1),
        pending_count: None,
        asset_count: None,
        directory_count: None,
    }
}

fn build_asset_details(
    mut asset: AssetSummary,
    dimensions: Option<oxy_media::ImageDimensions>,
    document: Result<MetadataDocument, crate::metadata::MetadataError>,
) -> AssetDetails {
    let sidecar_path = asset
        .has_sidecar
        .then(|| crate::files::sidecar_path(&asset.path));
    let (capture_metadata, focus_info, embedded_keywords, embedded_hierarchical_keywords) =
        document
            .as_ref()
            .map(|document| {
                (
                    document.capture.clone(),
                    document.focus.clone(),
                    document.embedded_editable.keywords.clone(),
                    document.embedded_editable.hierarchical_keywords.clone(),
                )
            })
            .unwrap_or_default();
    let (metadata, metadata_capability) = metadata_for_details(
        asset.kind,
        asset.has_sidecar,
        document.map(|document| document.editable),
    );
    asset.rating = metadata.rating;
    asset.color_label.clone_from(&metadata.color_label);
    asset.pick_label = metadata.pick_label;
    AssetDetails {
        asset,
        width: dimensions.map(|value| value.width),
        height: dimensions.map(|value| value.height),
        metadata,
        embedded_keywords,
        embedded_hierarchical_keywords,
        metadata_capability,
        sidecar_path,
        capture_metadata,
        focus_info,
    }
}

/// The native engine reads embedded metadata; sidecars override it. ExifTool
/// remains an optional write capability for explicit embedded synchronization.
fn metadata_for_details(
    _kind: AssetKind,
    has_sidecar: bool,
    result: Result<EditableMetadata, crate::metadata::MetadataError>,
) -> (EditableMetadata, MetadataCapability) {
    let provider = if has_sidecar {
        MetadataProvider::Sidecar
    } else {
        MetadataProvider::Native
    };
    match result {
        Ok(metadata) => (
            metadata,
            MetadataCapability {
                provider,
                readable: true,
                writable: true,
                detail: None,
            },
        ),
        Err(error) => (
            EditableMetadata::default(),
            MetadataCapability {
                provider,
                readable: false,
                writable: true,
                detail: Some(error.to_string()),
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_admission_does_not_hold_the_queue_or_delay_stop() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("waiting.jpg");
        std::fs::write(&path, b"source").unwrap();
        let runtime = CoreRuntime::default();
        let files = Arc::new(FsCatalog::default());
        let asset = files.get_asset(&path).unwrap();
        let library = Arc::new(Library::in_memory().unwrap());
        let queue = MetadataQueue::new(
            runtime.clone(),
            files,
            MetadataFacade::new(None),
            library.clone(),
            Arc::new(Tags::new(library.store())),
        );
        let store = library.store();
        let writer = store.write();
        let admission_queue = queue.clone();
        let admission_runtime = runtime.clone();
        let (finished, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let revision = metadata_source_revision(&asset);
            finished
                .send(admission_queue.enqueue(
                    &admission_runtime,
                    asset,
                    revision,
                    MetadataRequestPriority::Selected,
                    None,
                ))
                .unwrap();
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if queue
                .work
                .0
                .try_lock()
                .is_ok_and(|work| !work.admission_locks.is_empty())
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "admission did not start"
            );
            std::thread::yield_now();
        }
        assert!(queue.debug_snapshot().is_some());
        let stopping_runtime = runtime.clone();
        let (stopped, stop_complete) = mpsc::channel();
        let stopper = std::thread::spawn(move || {
            stopping_runtime.stop();
            stopped.send(()).unwrap();
        });
        stop_complete
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        drop(writer);
        assert!(
            result
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
        worker.join().unwrap();
        stopper.join().unwrap();
        assert!(runtime.shutdown(std::time::Duration::from_secs(2)));
    }

    #[test]
    fn asset_details_do_not_require_the_optional_exiftool_worker() {
        let (metadata, capability) = metadata_for_details(
            AssetKind::Heif,
            false,
            Err(crate::metadata::MetadataError::EmbeddedWorkerUnavailable),
        );

        assert_eq!(metadata, EditableMetadata::default());
        assert_eq!(capability.provider, MetadataProvider::Native);
        assert!(!capability.readable);
        assert!(capability.writable);
        assert!(capability.detail.is_some());
    }

    #[test]
    fn metadata_read_error_does_not_masquerade_as_missing_provider() {
        let (_, capability) = metadata_for_details(
            AssetKind::Heif,
            false,
            Err(crate::metadata::MetadataError::Read("invalid XMP".into())),
        );

        assert!(!capability.readable);
        assert!(capability.writable);
    }
}
