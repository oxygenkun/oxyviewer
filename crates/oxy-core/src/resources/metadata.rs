use super::projections::ResourceProjections;
use crate::browse::types::AssetSummary;
use crate::files::FsCatalog;
use crate::jobs::CoalescingPriorityQueue;
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
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, RwLock, Weak,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RequestKey {
    path: PathBuf,
    source_revision: String,
    generation: u64,
    path_generation: u64,
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
    pending: CoalescingPriorityQueue<RequestKey, Request, MetadataRequestPriority>,
    active: HashMap<RequestKey, Arc<Mutex<Request>>>,
    active_directory: Option<PathBuf>,
}

#[derive(Default)]
struct PathState {
    generation: u64,
    gate: Weak<Mutex<()>>,
}

struct CachedDetails {
    key: RequestKey,
    result: AssetDetailsResult,
}

#[derive(Clone)]
pub struct MetadataQueue {
    files: Arc<FsCatalog>,
    metadata: MetadataFacade,
    projections: Arc<ResourceProjections>,
    /// Tags own a sidecar's keyword vocabulary and assignment sources.
    tags: Arc<Tags>,
    work: Arc<(Mutex<WorkState>, Condvar)>,
    details: Arc<RwLock<HashMap<PathBuf, CachedDetails>>>,
    request_generation: Arc<AtomicU64>,
    paths: Arc<Mutex<HashMap<PathBuf, PathState>>>,
}

impl MetadataQueue {
    pub(crate) fn new(
        app: CoreRuntime,
        files: Arc<FsCatalog>,
        metadata: MetadataFacade,
        projections: Arc<ResourceProjections>,
        tags: Arc<Tags>,
    ) -> Self {
        let queue = Self {
            files,
            metadata,
            projections,
            tags,
            work: Arc::new((Mutex::new(WorkState::default()), Condvar::new())),
            details: Arc::new(RwLock::new(HashMap::new())),
            request_generation: Arc::new(AtomicU64::new(0)),
            paths: Arc::new(Mutex::new(HashMap::new())),
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
                .projections
                .metadata_projection_for_asset(&asset)
                .map_err(|error| error.to_string())?;
            if let Some(projection) = &optimistic {
                let _ = app.publish(CoreEvent::MetadataProjectionUpdated(projection.clone()));
            }
            let source_revision = metadata_source_revision(&asset);
            if let Some(cached) = self
                .projections
                .metadata_projection(&asset.path, &source_revision)
                .map_err(|error| error.to_string())?
                && cached.status == ResourceLoadStatus::Ready
            {
                // Projections written before keyword reconciliation was enabled
                // still need their sidecar imported on a cache hit.
                if asset.has_sidecar {
                    let editable = crate::metadata::read_sidecar(&asset.path)
                        .map_err(|error| error.to_string())?;
                    self.import_sidecar_tags(app, &asset, &editable);
                }
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

    fn import_sidecar_tags(
        &self,
        app: &CoreRuntime,
        asset: &AssetSummary,
        editable: &EditableMetadata,
    ) {
        if !asset.has_sidecar || app.is_stopping() {
            return;
        }
        match self.tags.import_sidecar_tags(
            &asset.path,
            &editable.keywords,
            &editable.hierarchical_keywords,
        ) {
            Ok(true) => {
                let _ = app.publish(CoreEvent::AssetTagsUpdated(asset.path.clone()));
            }
            Ok(false) => {}
            Err(error) => eprintln!("failed to import hierarchical tags: {error}"),
        }
    }

    pub fn invalidate_directory(&self, directory: &std::path::Path) {
        self.request_generation.fetch_add(1, Ordering::Relaxed);
        self.metadata.invalidate_summary_directory(directory);
        self.details
            .write()
            .expect("metadata details lock poisoned")
            .retain(|path, _| path.parent() != Some(directory));
    }

    /// Serialize admission/publication only for this path. The short map lock is
    /// released before filesystem/SQLite work; unrelated paths remain independent.
    fn with_path_gate<T>(&self, path: &Path, operation: impl FnOnce() -> T) -> T {
        let gate = {
            let mut paths = self.paths.lock().expect("metadata path state poisoned");
            let state = paths.entry(path.to_owned()).or_default();
            state.gate.upgrade().unwrap_or_else(|| {
                let gate = Arc::new(Mutex::new(()));
                state.gate = Arc::downgrade(&gate);
                gate
            })
        };
        let result = {
            let _guard = gate.lock().expect("metadata path gate poisoned");
            operation()
        };
        // Untouched paths need no permanent epoch entry. Keep changed epochs
        // while old requests may still carry an earlier generation.
        let mut paths = self.paths.lock().expect("metadata path state poisoned");
        if Arc::strong_count(&gate) == 1
            && paths.get(path).is_some_and(|state| state.generation == 0)
        {
            paths.remove(path);
        }
        result
    }

    fn path_generation(&self, path: &Path) -> u64 {
        self.paths
            .lock()
            .expect("metadata path state poisoned")
            .get(path)
            .map_or(0, |state| state.generation)
    }

    fn is_current(&self, key: &RequestKey) -> bool {
        key.generation == self.request_generation.load(Ordering::Relaxed)
            && key.path_generation == self.path_generation(&key.path)
    }

    pub(crate) fn invalidate_path(&self, path: &Path) {
        self.with_path_gate(path, || {
            self.paths
                .lock()
                .expect("metadata path state poisoned")
                .entry(path.to_owned())
                .or_default()
                .generation += 1;
            self.metadata.invalidate_summary_path(path);
            self.details
                .write()
                .expect("metadata details lock poisoned")
                .remove(path);
        });
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
            path_generation: self.path_generation(&asset.path),
        };
        let (sender, receiver) = mpsc::channel();
        if let Some(cached) = self
            .details
            .read()
            .expect("metadata details lock poisoned")
            .get(&key.path)
            .filter(|cached| cached.key == key)
            .map(|cached| cached.result.clone())
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
            path_generation: self.path_generation(&asset.path),
        };
        let path = key.path.clone();
        self.with_path_gate(&path, || {
            self.enqueue_admitted(app, asset, key, priority, detail_waiter)
        })
    }

    fn enqueue_admitted(
        &self,
        app: &CoreRuntime,
        asset: AssetSummary,
        key: RequestKey,
        priority: MetadataRequestPriority,
        detail_waiter: Option<Sender<AssetDetailsResult>>,
    ) -> Result<MetadataProjection, String> {
        if !self.is_current(&key) {
            return Err("metadata request invalidated during admission".into());
        }
        if let Some(waiter) = &detail_waiter
            && let Some(cached) = self
                .details
                .read()
                .expect("metadata details lock poisoned")
                .get(&key.path)
                .filter(|cached| cached.key == key)
                .map(|cached| cached.result.clone())
        {
            let projection = cached.metadata_projection.clone();
            let _ = waiter.send(cached);
            return Ok(projection);
        }
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
            drop(work);
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
            drop(work);
            return self
                .metadata
                .cached_projection(&asset)
                .ok_or_else(|| "pending metadata request has no projection".to_owned());
        }
        drop(work);

        let valid_at = self
            .projections
            .next_resource_revision()
            .map_err(|error| error.to_string())?;
        let observation = self.metadata.observe_asset_at(&asset, valid_at);
        if let Some(cached) = self
            .projections
            .metadata_projection(&asset.path, &key.source_revision)
            .map_err(|error| error.to_string())?
        {
            self.metadata.restore_projection(&observation, &cached);
        } else if let Some(cached) = self
            .projections
            .metadata_projection_for_asset(&asset)
            .map_err(|error| error.to_string())?
        {
            self.metadata.restore_projection(&observation, &cached);
        }
        let loading = self
            .projections
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
            || !self.is_current(&key)
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

    fn requeue_waiters(&self, app: &CoreRuntime, key: &RequestKey, request: &Arc<Mutex<Request>>) {
        let waiters = {
            let mut work = self.work.0.lock().expect("metadata queue lock poisoned");
            work.active.remove(key);
            std::mem::take(
                &mut request
                    .lock()
                    .expect("active metadata request lock poisoned")
                    .detail_waiters,
            )
        };
        if waiters.is_empty() || app.is_stopping() {
            return;
        }
        let Ok(asset) = self.files.get_asset(&key.path) else {
            return;
        };
        let revision = metadata_source_revision(&asset);
        for waiter in waiters {
            let _ = self.enqueue(
                app,
                asset.clone(),
                revision.clone(),
                MetadataRequestPriority::Selected,
                Some(waiter),
            );
        }
    }

    fn process_request(
        &self,
        app: &CoreRuntime,
        key: &RequestKey,
        request: &Arc<Mutex<Request>>,
    ) -> bool {
        if !self.is_current(key) {
            return false;
        }
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
            let read = self.metadata.read_editable_observation(&observation);
            if !self.is_current(key) {
                return false;
            }
            let candidate = match read {
                Ok((editable, projection)) => {
                    self.import_sidecar_tags(app, &asset, &editable);
                    projection
                }
                Err(error) => self
                    .metadata
                    .fail_observation(&observation, error.to_string()),
            };
            if app.is_stopping() {
                return true;
            }
            if !self.is_current(key) {
                return false;
            }
            let projection = match self.projections.accept_metadata_projection(candidate) {
                Ok(projection) => projection,
                Err(error) => {
                    eprintln!("failed to persist metadata projection: {error}");
                    self.metadata
                        .fail_observation(&observation, error.to_string())
                }
            };
            if projection.error.as_deref() != Some("invalidated") {
                let _ = app.publish(CoreEvent::MetadataProjectionUpdated(projection));
            }
            let should_upgrade = {
                let mut work = self.work.0.lock().expect("metadata queue lock poisoned");
                let should_upgrade = !request
                    .lock()
                    .expect("active metadata request lock poisoned")
                    .detail_waiters
                    .is_empty();
                if !should_upgrade {
                    work.active.remove(key);
                }
                should_upgrade
            };
            if !should_upgrade {
                return true;
            }
        }

        let dimensions = oxy_media::dimensions(&asset.path, asset.kind).ok();
        let display_dimensions = dimensions.map(|value| (value.width, value.height));
        let document = self
            .metadata
            .read_document_observation(&observation, display_dimensions);
        if app.is_stopping() {
            return true;
        }
        if !self.is_current(key) {
            return false;
        }
        if let Ok((document, _)) = &document {
            self.import_sidecar_tags(app, &asset, &document.editable);
        }
        let (document, candidate) = match document {
            Ok((document, projection)) => (Ok(document), projection),
            Err(error) => {
                let projection = self
                    .metadata
                    .fail_observation(&observation, error.to_string());
                (Err(error), projection)
            }
        };
        let projection = match self.projections.accept_metadata_projection(candidate) {
            Ok(projection) => projection,
            Err(error) => {
                eprintln!("failed to persist metadata projection: {error}");
                self.metadata
                    .fail_observation(&observation, error.to_string())
            }
        };
        let details = build_asset_details(asset, dimensions, document);
        let result = AssetDetailsResult {
            details,
            metadata_projection: projection.clone(),
        };
        self.details
            .write()
            .expect("metadata details lock poisoned")
            .insert(
                key.path.clone(),
                CachedDetails {
                    key: key.clone(),
                    result: result.clone(),
                },
            );
        if projection.error.as_deref() != Some("invalidated") {
            let _ = app.publish(CoreEvent::MetadataProjectionUpdated(projection));
        }
        let waiters = {
            let mut work = self.work.0.lock().expect("metadata queue lock poisoned");
            let waiters = std::mem::take(
                &mut request
                    .lock()
                    .expect("active metadata request lock poisoned")
                    .detail_waiters,
            );
            work.active.remove(key);
            waiters
        };
        for waiter in waiters {
            let _ = waiter.send(result.clone());
        }
        true
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
                    let current = queue
                        .with_path_gate(&key.path, || queue.process_request(&app, &key, &request));
                    if !current {
                        queue.requeue_waiters(&app, &key, &request);
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
    fn invalidated_pending_details_are_reobserved_and_waiters_are_released() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.jpg");
        image::RgbImage::new(8, 8).save(&path).unwrap();
        let runtime = CoreRuntime::default();
        let files = Arc::new(FsCatalog::default());
        let metadata = MetadataFacade::default();
        metadata
            .patch_metadata(
                &path,
                AssetKind::Jpeg,
                &crate::metadata::types::MetadataPatch {
                    rating: Some(Some(1)),
                    ..crate::metadata::types::MetadataPatch::default()
                },
            )
            .unwrap();
        let store = Arc::new(crate::storage::Store::in_memory().unwrap());
        let projections = Arc::new(ResourceProjections::new(Arc::clone(&store)));
        let queue = MetadataQueue::new(
            runtime.clone(),
            Arc::clone(&files),
            metadata.clone(),
            Arc::clone(&projections),
            Arc::new(Tags::new(store)),
        );
        let asset = files.get_asset(&path).unwrap();
        let key = RequestKey {
            path: path.clone(),
            source_revision: metadata_source_revision(&asset),
            generation: 0,
            path_generation: 0,
        };
        let observation =
            metadata.observe_asset_at(&asset, projections.next_resource_revision().unwrap());
        metadata
            .patch_metadata(
                &path,
                AssetKind::Jpeg,
                &crate::metadata::types::MetadataPatch {
                    rating: Some(Some(5)),
                    ..crate::metadata::types::MetadataPatch::default()
                },
            )
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        {
            let mut work = queue.work.0.lock().unwrap();
            work.pending.push_or_merge(
                key,
                Request {
                    asset,
                    observation,
                    detail_waiters: vec![sender],
                    priority: MetadataRequestPriority::Selected,
                },
                MetadataRequestPriority::Selected,
                |_, _| unreachable!(),
            );
            queue.invalidate_path(&path);
        }
        assert_eq!(queue.request_generation.load(Ordering::Relaxed), 0);
        queue.work.1.notify_one();
        let result = receiver
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert_eq!(result.details.metadata.rating, Some(5));
        assert_eq!(result.metadata_projection.rating, Some(5));
        assert!(runtime.shutdown(std::time::Duration::from_secs(3)));
    }

    #[test]
    fn summaries_import_sidecar_markings_and_tags_without_native_image_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rated.HIF");
        std::fs::write(&path, b"unreadable image metadata").unwrap();
        let sidecar = directory.path().join("rated.xmp");
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
          <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/"
              xmlns:dc="http://purl.org/dc/elements/1.1/"
              xmlns:lr="http://ns.adobe.com/lightroom/1.0/" xmp:Rating="3">
              <dc:subject><rdf:Bag><rdf:li>腋</rdf:li></rdf:Bag></dc:subject>
              <lr:hierarchicalSubject><rdf:Bag><rdf:li>xp|腋</rdf:li></rdf:Bag></lr:hierarchicalSubject>
            </rdf:Description>
          </rdf:RDF>
        </x:xmpmeta>"#;
        std::fs::write(&sidecar, xml).unwrap();
        let (runtime, events) = CoreRuntime::channel();
        let store = Arc::new(crate::storage::Store::in_memory().unwrap());
        let tags = Arc::new(Tags::new(store.clone()));
        let projections = Arc::new(ResourceProjections::new(store));
        let queue = MetadataQueue::new(
            runtime.clone(),
            Arc::new(FsCatalog::default()),
            MetadataFacade::default(),
            projections,
            tags.clone(),
        );
        queue
            .request_summaries(
                &runtime,
                vec![path.clone()],
                MetadataRequestPriority::Visible,
            )
            .unwrap();
        let mut imported = false;
        loop {
            match events
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap()
            {
                CoreEvent::AssetTagsUpdated(updated) => {
                    assert_eq!(updated, path);
                    imported = true;
                }
                CoreEvent::MetadataProjectionUpdated(projection)
                    if projection.status != ResourceLoadStatus::Loading =>
                {
                    assert_eq!(projection.status, ResourceLoadStatus::Ready);
                    assert_eq!(projection.rating, Some(3));
                    break;
                }
                _ => {}
            }
        }
        assert!(imported);
        let assignments = tags
            .asset_tag_assignments(std::slice::from_ref(&path))
            .unwrap();
        let assigned = assignments
            .iter()
            .filter(|item| item.assigned_count > 0)
            .collect::<Vec<_>>();
        assert_eq!(assigned.len(), 1);
        assert_eq!(assigned[0].tag.path, "xp|腋");
        let cached = queue
            .request_summaries(
                &runtime,
                vec![path.clone()],
                MetadataRequestPriority::Visible,
            )
            .unwrap();
        assert_eq!(cached[0].rating, Some(3));
        assert!(
            !events
                .try_iter()
                .any(|event| matches!(event, CoreEvent::AssetTagsUpdated(_)))
        );

        // Selecting the same file must retain its sidecar markings even when
        // the embedded capture metadata cannot be read.
        let details = queue
            .request_details(&runtime, path.clone())
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert_eq!(details.metadata_projection.rating, Some(3));
        assert_eq!(details.details.metadata.rating, Some(3));
        assert!(details.details.metadata_capability.readable);
        assert_eq!(std::fs::read_to_string(sidecar).unwrap(), xml);
        assert_eq!(std::fs::read(path).unwrap(), b"unreadable image metadata");
        assert!(runtime.shutdown(std::time::Duration::from_secs(3)));
    }

    #[test]
    fn cached_summary_still_imports_sidecar_tags() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cached.HIF");
        std::fs::write(&path, b"source").unwrap();
        std::fs::write(directory.path().join("cached.xmp"),
            r#"<rdf:Description xmp:Rating="5"><dc:subject><rdf:Bag><rdf:li>cached tag</rdf:li></rdf:Bag></dc:subject></rdf:Description>"#).unwrap();
        let (runtime, events) = CoreRuntime::channel();
        let files = Arc::new(FsCatalog::default());
        let store = Arc::new(crate::storage::Store::in_memory().unwrap());
        let tags = Arc::new(Tags::new(store.clone()));
        let projections = Arc::new(ResourceProjections::new(store));
        let metadata = MetadataFacade::default();
        let observation = metadata.observe_asset(&files.get_asset(&path).unwrap());
        projections
            .accept_metadata_projection(metadata.read_summary_observation(&observation).unwrap())
            .unwrap();
        let queue = MetadataQueue::new(runtime.clone(), files, metadata, projections, tags.clone());
        let snapshots = queue
            .request_summaries(
                &runtime,
                vec![path.clone()],
                MetadataRequestPriority::Visible,
            )
            .unwrap();
        assert_eq!(snapshots[0].rating, Some(5));
        assert!(
            events
                .try_iter()
                .any(|event| matches!(event, CoreEvent::AssetTagsUpdated(_)))
        );
        assert!(
            tags.asset_tag_assignments(&[path])
                .unwrap()
                .iter()
                .any(|item| item.tag.name == "cached tag" && item.assigned_count == 1)
        );
        assert!(runtime.shutdown(std::time::Duration::from_secs(3)));
    }

    #[test]
    fn blocked_admission_does_not_hold_the_queue_or_delay_stop() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("waiting.jpg");
        std::fs::write(&path, b"source").unwrap();
        let runtime = CoreRuntime::default();
        let files = Arc::new(FsCatalog::default());
        let asset = files.get_asset(&path).unwrap();
        let store = Arc::new(crate::storage::Store::in_memory().unwrap());
        let projections = Arc::new(ResourceProjections::new(store.clone()));
        let queue = MetadataQueue::new(
            runtime.clone(),
            files,
            MetadataFacade::new(None),
            projections,
            Arc::new(Tags::new(store.clone())),
        );
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
                .paths
                .try_lock()
                .is_ok_and(|paths| paths.values().any(|state| state.gate.strong_count() > 0))
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
