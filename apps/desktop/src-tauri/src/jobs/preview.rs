use crate::state::cache::CacheManager;
use oxy_domain::{
    AssetKind, DebugQueueItem, DebugQueueState, ImageProjection, MediaPersistence,
    MediaResourceDescriptor, MediaSatisfaction, OmittedScheduleAction, PreviewKind,
    PreviewOmittedPolicy, PreviewPriority, PreviewResult, PreviewScheduleIntent, RenderLevel,
    ResourceLoadStatus, SchedulePlacement,
};
use oxy_library::Library;
use oxy_runtime::{
    CancellationToken, CoalescingPriorityQueue, EffectiveScheduleChange, OmittedIntentPolicy,
    QueuePlacement, SchedulePosition, ScopedIntentScheduler,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

pub const IMAGE_PROJECTION_UPDATED_EVENT: &str = "image-projection-updated";
const PREVIEW_WORKER_COUNT: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RequestKey {
    path: PathBuf,
    source_revision: String,
    level: RenderLevel,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewScheduleKey {
    pub path: PathBuf,
    pub level: RenderLevel,
}

struct WorkRequest {
    path: PathBuf,
    preview_dir: PathBuf,
    kind: AssetKind,
    source_revision: String,
    level: RenderLevel,
    valid_at: u64,
    waiters: Vec<Waiter>,
    cancellation: CancellationToken,
}

impl WorkRequest {
    fn remove_waiter(&mut self, request_id: &str) -> bool {
        let previous_len = self.waiters.len();
        self.waiters.retain(|waiter| waiter.id != request_id);
        let removed = self.waiters.len() != previous_len;
        if removed && self.waiters.is_empty() {
            self.cancellation.cancel();
        }
        removed
    }
}

struct Waiter {
    id: String,
    sender: Sender<Result<ImageProjection, String>>,
    interim_delivered: bool,
}

pub struct PreviewRequest {
    pub request_id: String,
    pub path: PathBuf,
    pub preview_dir: PathBuf,
    pub kind: AssetKind,
    pub size_bytes: u64,
    pub modified_at_ms: u64,
    pub level: RenderLevel,
    pub priority: PreviewPriority,
    pub queue_order: usize,
}

pub struct PreviewIdentity {
    pub path: PathBuf,
    pub level: RenderLevel,
}

struct ProjectionUpdate {
    source_revision: String,
    level: RenderLevel,
    valid_at: u64,
    status: ResourceLoadStatus,
    result: Option<PreviewResult>,
    error: Option<String>,
}

#[derive(Default)]
struct WorkState {
    pending: CoalescingPriorityQueue<RequestKey, WorkRequest, SchedulePosition>,
    pending_keys: HashMap<PreviewScheduleKey, HashSet<RequestKey>>,
    active: HashMap<RequestKey, Arc<Mutex<WorkRequest>>>,
    schedule: ScopedIntentScheduler<PreviewScheduleKey, String>,
    active_directory: Option<PathBuf>,
    cancelled_before_admission: VecDeque<(String, Instant)>,
}

impl WorkState {
    fn remember_early_cancellation(&mut self, request_id: &str) {
        self.expire_early_cancellations();
        // Commands may still be observing source metadata when cancellation
        // arrives. Bound these short tombstones; request IDs are unique UUIDs.
        if self.cancelled_before_admission.len() == 1024 {
            self.cancelled_before_admission.pop_front();
        }
        self.cancelled_before_admission
            .push_back((request_id.to_owned(), Instant::now()));
    }

    fn expire_early_cancellations(&mut self) {
        while self
            .cancelled_before_admission
            .front()
            .is_some_and(|(_, at)| at.elapsed() >= Duration::from_secs(120))
        {
            self.cancelled_before_admission.pop_front();
        }
    }

    fn take_early_cancellation(&mut self, request_id: &str) -> bool {
        self.expire_early_cancellations();
        let Some(index) = self
            .cancelled_before_admission
            .iter()
            .position(|(id, _)| id == request_id)
        else {
            return false;
        };
        self.cancelled_before_admission.remove(index);
        true
    }

    fn apply_schedule_changes(
        &mut self,
        changes: Vec<EffectiveScheduleChange<PreviewScheduleKey>>,
    ) {
        for change in changes {
            let Some(position) = change.position else {
                continue;
            };
            let Some(request_keys) = self.pending_keys.get(&change.key).cloned() else {
                continue;
            };
            for request_key in request_keys {
                self.pending.reprioritize_if_present(&request_key, position);
            }
        }
    }
}

#[derive(Clone)]
pub struct PreviewQueue {
    work: Arc<(Mutex<WorkState>, Condvar)>,
    projections: Arc<RwLock<HashMap<(PathBuf, RenderLevel), ImageProjection>>>,
    library: Arc<Library>,
    cache: Arc<CacheManager>,
    request_generation: Arc<AtomicU64>,
}

impl PreviewQueue {
    pub fn new(app: AppHandle, library: Arc<Library>, cache: Arc<CacheManager>) -> Self {
        let queue = Self {
            work: Arc::new((Mutex::new(WorkState::default()), Condvar::new())),
            projections: Arc::new(RwLock::new(HashMap::new())),
            library,
            cache,
            request_generation: Arc::new(AtomicU64::new(0)),
        };
        for worker_index in 0..PREVIEW_WORKER_COUNT {
            queue.spawn_worker(app.clone(), worker_index);
        }
        queue
    }

    fn restore_cached_projection(
        &self,
        cached: ImageProjection,
        preview_dir: &std::path::Path,
    ) -> Result<ImageProjection, String> {
        let key = (cached.path.clone(), cached.level);
        // Source and artifact validation can involve filesystem I/O. Keep it
        // outside the global projection lock, then fence its publication.
        let cached = self
            .projections
            .read()
            .expect("image projection lock poisoned")
            .get(&key)
            .filter(|current| current.projection_revision >= cached.projection_revision)
            .cloned()
            .unwrap_or(cached);
        let old_resource = cached
            .result
            .as_ref()
            .and_then(|result| result.resource.clone());
        let mut restored = restore_projection_resource(cached, preview_dir)?;
        let new_resource = restored
            .result
            .as_ref()
            .and_then(|result| result.resource.clone());
        let mut projections = self
            .projections
            .write()
            .expect("image projection lock poisoned");
        if let Some(current) = projections.get(&key)
            && current.projection_revision > restored.projection_revision
        {
            if let Some(resource) = new_resource
                && current
                    .result
                    .as_ref()
                    .and_then(|result| result.resource.as_ref())
                    != Some(&resource)
            {
                oxy_media::shared_resource_registry().release(&resource.resource_id);
            }
            return Ok(current.clone());
        }
        if new_resource != old_resource {
            // Descriptor replacement is an observable projection change. Use
            // the existing authoritative sequence, preserving validAt fences.
            restored = self
                .library
                .accept_image_projection(restored)
                .map_err(|error| error.to_string())?;
            if let Some(resource) = new_resource
                && restored
                    .result
                    .as_ref()
                    .and_then(|result| result.resource.as_ref())
                    != Some(&resource)
            {
                oxy_media::shared_resource_registry().release(&resource.resource_id);
            }
        }
        projections.insert(key, restored.clone());
        Ok(restored)
    }

    pub fn request(
        &self,
        app: &AppHandle,
        request: PreviewRequest,
    ) -> Result<(ImageProjection, Receiver<Result<ImageProjection, String>>), String> {
        let PreviewRequest {
            request_id,
            path,
            preview_dir,
            kind,
            size_bytes,
            modified_at_ms,
            level,
            priority,
            queue_order,
        } = request;
        let requested_position = schedule_position(priority, queue_order);
        let source_revision =
            source_revision(&path, modified_at_ms, size_bytes, &preview_dir, kind, level)?;
        let state_key = (path.clone(), level);
        let memory_projection = self
            .projections
            .read()
            .expect("image projection lock poisoned")
            .get(&state_key)
            .filter(|projection| projection.source_revision == source_revision)
            .cloned();
        if let Some(cached) = memory_projection {
            let cached = self.restore_cached_projection(cached, &preview_dir)?;
            if projection_is_terminal(&cached)
                && cached.result.as_ref().is_some_and(preview_result_is_live)
            {
                let (sender, receiver) = mpsc::channel();
                let _ = sender.send(Ok(cached.clone()));
                return Ok((cached, receiver));
            }
        }
        if let Some(cached) = self
            .library
            .image_projection(&path, level, &source_revision)
            .map_err(|error| error.to_string())?
        {
            let cached = self.restore_cached_projection(cached, &preview_dir)?;
            if projection_is_terminal(&cached)
                && cached.result.as_ref().is_some_and(preview_result_is_live)
            {
                let (sender, receiver) = mpsc::channel();
                let _ = sender.send(Ok(cached.clone()));
                return Ok((cached, receiver));
            }
        }

        let (sender, receiver) = mpsc::channel();
        let key = RequestKey {
            path: path.clone(),
            source_revision: source_revision.clone(),
            level,
            generation: self.request_generation.load(Ordering::Relaxed),
        };
        let schedule_key = PreviewScheduleKey {
            path: path.clone(),
            level,
        };
        let mut work = self.work.0.lock().expect("preview queue lock poisoned");
        if work.take_early_cancellation(&request_id) {
            return Err("preview request was cancelled before admission".into());
        }
        if work
            .active_directory
            .as_deref()
            .is_some_and(|directory| path.parent() != Some(directory))
        {
            return Err("preview request left the active directory".into());
        }
        work.schedule.reconcile(
            request_id.clone(),
            0,
            [(schedule_key.clone(), requested_position)],
            OmittedIntentPolicy::Release,
        );
        let effective_position = work
            .schedule
            .effective_position(&schedule_key)
            .unwrap_or(requested_position);
        if let Some(active) = work.active.get(&key) {
            active
                .lock()
                .expect("active preview request lock poisoned")
                .waiters
                .push(Waiter {
                    id: request_id,
                    sender,
                    interim_delivered: false,
                });
            let projection = self
                .projections
                .read()
                .expect("image projection lock poisoned")
                .get(&state_key)
                .cloned()
                .expect("active preview request must have a projection");
            return Ok((projection, receiver));
        }
        if work.pending.contains_key(&key) {
            let projection = self
                .projections
                .read()
                .expect("image projection lock poisoned")
                .get(&state_key)
                .cloned()
                .expect("pending preview request must have a projection");
            let updated = work.pending.update_priority_if_present(&key, |current| {
                current.waiters.push(Waiter {
                    id: request_id,
                    sender,
                    interim_delivered: false,
                });
                effective_position
            });
            debug_assert!(updated);
            return Ok((projection, receiver));
        }

        let valid_at = match self.library.next_resource_revision() {
            Ok(valid_at) => valid_at,
            Err(error) => {
                let changes = work.schedule.release_scope(&request_id);
                work.apply_schedule_changes(changes);
                return Err(error.to_string());
            }
        };
        let loading = match self.transition(
            path.clone(),
            ProjectionUpdate {
                source_revision: source_revision.clone(),
                level,
                valid_at,
                status: ResourceLoadStatus::Loading,
                result: None,
                error: None,
            },
        ) {
            Ok(loading) => loading,
            Err(error) => {
                let changes = work.schedule.release_scope(&request_id);
                work.apply_schedule_changes(changes);
                return Err(error);
            }
        };
        let _ = app.emit(IMAGE_PROJECTION_UPDATED_EVENT, loading.clone());
        let request = WorkRequest {
            path,
            preview_dir,
            kind,
            source_revision,
            level,
            valid_at,
            waiters: vec![Waiter {
                id: request_id,
                sender,
                interim_delivered: false,
            }],
            cancellation: CancellationToken::default(),
        };
        work.pending.push_or_merge(
            key.clone(),
            request,
            effective_position,
            |current, replacement| {
                current.waiters.extend(replacement.waiters);
            },
        );
        work.pending_keys
            .entry(schedule_key)
            .or_default()
            .insert(key);
        drop(work);
        self.work.1.notify_one();
        Ok((loading, receiver))
    }

    pub fn cancel_request(&self, identity: PreviewIdentity, request_id: &str) -> bool {
        let schedule_key = PreviewScheduleKey {
            path: identity.path,
            level: identity.level,
        };
        let mut work = self.work.0.lock().expect("preview queue lock poisoned");
        let changes = work.schedule.release_scope(&request_id.to_owned());
        let mut changed = !changes.is_empty();
        work.apply_schedule_changes(changes);
        let remaining_position = work
            .schedule
            .effective_position(&schedule_key)
            .unwrap_or_else(|| schedule_position(PreviewPriority::Preload, usize::MAX));

        let pending_keys = work
            .pending_keys
            .get(&schedule_key)
            .cloned()
            .unwrap_or_default();
        for key in pending_keys {
            let mut remove_key = false;
            let updated = work.pending.update_or_remove_if_present(&key, |request| {
                let previous_len = request.waiters.len();
                request.waiters.retain(|waiter| waiter.id != request_id);
                changed |= request.waiters.len() != previous_len;
                remove_key = request.waiters.is_empty();
                (!remove_key).then_some(remaining_position)
            });
            if updated
                && remove_key
                && let Some(keys) = work.pending_keys.get_mut(&schedule_key)
            {
                keys.remove(&key);
                if keys.is_empty() {
                    work.pending_keys.remove(&schedule_key);
                }
            }
        }

        for (key, request) in &work.active {
            if key.path != schedule_key.path || key.level != schedule_key.level {
                continue;
            }
            let mut request = request
                .lock()
                .expect("active preview request lock poisoned");
            changed |= request.remove_waiter(request_id);
        }
        if !changed {
            work.remember_early_cancellation(request_id);
        }
        changed
    }

    pub fn reconcile_schedule(
        &self,
        scope_id: String,
        epoch: u64,
        intents: Vec<PreviewScheduleIntent>,
        omitted: PreviewOmittedPolicy,
    ) -> Result<bool, String> {
        let intents = intents
            .into_iter()
            .map(|intent| {
                (
                    PreviewScheduleKey {
                        path: intent.path,
                        level: intent.level,
                    },
                    schedule_position(intent.priority, intent.rank),
                )
            })
            .collect::<Vec<_>>();
        let omitted = omitted_policy(omitted)?;
        let mut work = self.work.0.lock().expect("preview queue lock poisoned");
        let Some(changes) = work.schedule.reconcile(scope_id, epoch, intents, omitted) else {
            return Ok(false);
        };
        work.apply_schedule_changes(changes);
        Ok(true)
    }

    pub fn upsert_schedule(
        &self,
        scope_id: String,
        epoch: u64,
        key: PreviewScheduleKey,
        priority: PreviewPriority,
        placement: SchedulePlacement,
    ) -> bool {
        let tier = schedule_position(priority, 0).tier;
        let placement = runtime_placement(placement);
        let mut work = self.work.0.lock().expect("preview queue lock poisoned");
        let Some(changes) = work.schedule.upsert(scope_id, epoch, key, tier, placement) else {
            return false;
        };
        work.apply_schedule_changes(changes);
        true
    }

    pub fn release_schedule(&self, scope_id: &str, epoch: u64, key: &PreviewScheduleKey) -> bool {
        let mut work = self.work.0.lock().expect("preview queue lock poisoned");
        let Some(changes) = work.schedule.release(&scope_id.to_owned(), epoch, key) else {
            return false;
        };
        work.apply_schedule_changes(changes);
        true
    }

    /// Drops work that has not started for assets outside the directory the
    /// user is currently viewing. Active decodes are allowed to finish.
    pub fn clear_pending_outside_directory(&self, directory: &std::path::Path) -> usize {
        let mut work = self.work.0.lock().expect("preview queue lock poisoned");
        work.active_directory = Some(directory.to_owned());
        let removed = work
            .pending
            .remove_if(|key, _| key.path.parent() != Some(directory));
        for (key, request, _) in &removed {
            let schedule_key = PreviewScheduleKey {
                path: key.path.clone(),
                level: key.level,
            };
            if let Some(keys) = work.pending_keys.get_mut(&schedule_key) {
                keys.remove(key);
                if keys.is_empty() {
                    work.pending_keys.remove(&schedule_key);
                }
            }
            for waiter in &request.waiters {
                let changes = work.schedule.release_scope(&waiter.id);
                work.apply_schedule_changes(changes);
            }
        }
        let removed_count = removed.len();
        drop(work);
        for (_, request, _) in removed {
            for waiter in request.waiters {
                let _ = waiter
                    .sender
                    .send(Err("preview request left the active directory".into()));
            }
        }
        removed_count
    }

    pub fn invalidate_directory(&self, directory: &std::path::Path) {
        self.request_generation.fetch_add(1, Ordering::Relaxed);
        self.projections
            .write()
            .expect("image projection lock poisoned")
            .retain(|(path, _), _| path.parent() != Some(directory));
    }

    pub fn invalidate_all(&self) {
        self.request_generation.fetch_add(1, Ordering::Relaxed);
        if let Err(error) = self.library.invalidate_image_projections() {
            eprintln!("failed to invalidate persisted image projections: {error}");
        }
        self.projections
            .write()
            .expect("image projection lock poisoned")
            .clear();
    }

    pub fn debug_snapshot(&self) -> DebugQueueState {
        let work = self.work.0.lock().expect("preview queue lock poisoned");
        let pending = work
            .pending
            .entries()
            .map(|(key, request, position)| debug_item(key, request, position))
            .collect::<Vec<_>>();
        let active = work
            .active
            .iter()
            .map(|(key, request)| {
                let request = request
                    .lock()
                    .expect("active preview request lock poisoned");
                let schedule_key = PreviewScheduleKey {
                    path: key.path.clone(),
                    level: key.level,
                };
                let position = work
                    .schedule
                    .effective_position(&schedule_key)
                    .unwrap_or_else(|| schedule_position(PreviewPriority::Preload, usize::MAX));
                debug_item(key, &request, position)
            })
            .collect();
        DebugQueueState {
            name: "preview".into(),
            concurrency: PREVIEW_WORKER_COUNT,
            pending,
            active,
        }
    }

    fn transition(
        &self,
        path: PathBuf,
        update: ProjectionUpdate,
    ) -> Result<ImageProjection, String> {
        let ProjectionUpdate {
            source_revision,
            level,
            valid_at,
            status,
            result,
            error,
        } = update;
        let key = (path.clone(), level);
        let mut projections = self
            .projections
            .write()
            .expect("image projection lock poisoned");
        if let Some(current) = projections
            .get(&key)
            .filter(|current| current.valid_at > valid_at)
        {
            return Ok(current.clone());
        }
        let retained_result = result.or_else(|| {
            projections
                .get(&key)
                .filter(|current| current.source_revision == source_revision)
                .and_then(|current| current.result.clone())
        });
        let projection = ImageProjection {
            path,
            source_revision,
            projection_revision: 0,
            valid_at,
            status,
            level,
            result: retained_result,
            error,
        };
        let projection = self
            .library
            .accept_image_projection(projection)
            .map_err(|error| error.to_string())?;
        projections.insert(key, projection.clone());
        Ok(projection)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_interim_upgrade(
        &self,
        app: &AppHandle,
        path: &std::path::Path,
        preview_dir: &std::path::Path,
        kind: AssetKind,
        source_revision: &str,
        level: RenderLevel,
        valid_at: u64,
        priority: PreviewPriority,
        cancellation: &CancellationToken,
    ) -> Result<Option<ImageProjection>, String> {
        // Thumbnail Interim is the terminal display contract: this level has no
        // native upgrade phase, and the frontend settles it as displayable.
        if level == RenderLevel::Thumbnail || cancellation.is_cancelled() {
            return Ok(None);
        }
        let upgrade = match oxy_media::preview_for_app_upgrade(
            path,
            preview_dir,
            level,
            priority,
            kind,
            cancellation,
        ) {
            Ok(upgrade) => upgrade,
            Err(_) if cancellation.is_cancelled() => return Ok(None),
            Err(error) => {
                let message = error.to_string();
                let projection = self.transition_interim_upgrade_error(
                    path,
                    source_revision,
                    level,
                    valid_at,
                    message.clone(),
                )?;
                let _ = app.emit(IMAGE_PROJECTION_UPDATED_EVENT, projection);
                return Err(message);
            }
        };
        if upgrade.result.satisfaction != Some(MediaSatisfaction::Satisfied) {
            let message = "preview upgrade did not produce a satisfied artifact".to_owned();
            let projection = self.transition_interim_upgrade_error(
                path,
                source_revision,
                level,
                valid_at,
                message.clone(),
            )?;
            let _ = app.emit(IMAGE_PROJECTION_UPDATED_EVENT, projection);
            return Err(message);
        }
        let resource_id = upgrade
            .result
            .resource
            .as_ref()
            .map(|resource| resource.resource_id.clone());
        let projection = self.transition(
            path.to_owned(),
            ProjectionUpdate {
                source_revision: source_revision.to_owned(),
                level,
                valid_at,
                status: ResourceLoadStatus::Ready,
                result: Some(upgrade.result),
                error: None,
            },
        );
        let projection = projection?;
        let _ = app.emit(IMAGE_PROJECTION_UPDATED_EVENT, projection.clone());
        for completion in upgrade.completions {
            if resource_id.is_some() {
                self.spawn_persistence_completion(
                    app.clone(),
                    path.to_owned(),
                    source_revision.to_owned(),
                    level,
                    valid_at,
                    completion,
                );
            }
        }
        Ok(Some(projection))
    }

    fn transition_interim_upgrade_error(
        &self,
        path: &std::path::Path,
        source_revision: &str,
        level: RenderLevel,
        valid_at: u64,
        error: String,
    ) -> Result<ImageProjection, String> {
        self.transition(
            path.to_owned(),
            ProjectionUpdate {
                source_revision: source_revision.to_owned(),
                level,
                valid_at,
                status: ResourceLoadStatus::Error,
                result: None,
                error: Some(error),
            },
        )
    }

    fn spawn_persistence_completion(
        &self,
        app: AppHandle,
        path: PathBuf,
        source_revision: String,
        level: RenderLevel,
        valid_at: u64,
        completion: Receiver<oxy_media::PersistenceCompletion>,
    ) {
        let queue = self.clone();
        std::thread::spawn(move || {
            let Ok(completion) = completion.recv() else {
                return;
            };
            let (resource_id, managed_path, persistence) = match completion {
                oxy_media::PersistenceCompletion::Persisted { resource_id, path } => {
                    (resource_id, Some(path), MediaPersistence::Persisted)
                }
                oxy_media::PersistenceCompletion::Failed { resource_id, .. } => {
                    (resource_id, None, MediaPersistence::Skipped)
                }
            };
            let current = queue
                .projections
                .read()
                .expect("image projection lock poisoned")
                .get(&(path.clone(), level))
                .filter(|projection| {
                    projection.source_revision == source_revision
                        && projection.valid_at == valid_at
                        && projection.result.as_ref().and_then(|result| {
                            result
                                .resource
                                .as_ref()
                                .map(|resource| &resource.resource_id)
                        }) == Some(&resource_id)
                })
                .cloned();
            let Some(current) = current else {
                return;
            };
            let Some(mut result) = current.result else {
                return;
            };
            if let Some(managed_path) = managed_path {
                result.path = managed_path.clone();
                if queue.cache.try_start_prune() {
                    let cache = Arc::clone(&queue.cache);
                    std::thread::spawn(move || {
                        loop {
                            if let Err(error) = cache.prune_after_write(&managed_path) {
                                eprintln!("preview cache pruning failed: {error}");
                            }
                            if !cache.finish_prune() {
                                break;
                            }
                        }
                    });
                }
            }
            result.persistence = Some(persistence);
            if let Ok(projection) = queue.transition(
                path,
                ProjectionUpdate {
                    source_revision,
                    level,
                    valid_at,
                    status: ResourceLoadStatus::Ready,
                    result: Some(result),
                    error: None,
                },
            ) {
                let _ = app.emit(IMAGE_PROJECTION_UPDATED_EVENT, projection);
            }
        });
    }

    fn spawn_worker(&self, app: AppHandle, worker_index: usize) {
        let queue = self.clone();
        std::thread::Builder::new()
            .name(format!("oxy-image-projection-{worker_index}"))
            .spawn(move || {
                loop {
                    let request = {
                        let mut pending = queue.work.0.lock().expect("preview queue lock poisoned");
                        while pending.pending.is_empty() {
                            pending = queue
                                .work
                                .1
                                .wait(pending)
                                .expect("preview queue lock poisoned");
                        }
                        // Leave background requests in the priority queue so
                        // a newly visible request can wake and pass them.
                        while (queue.library.foreground.is_busy() || !pending.active.is_empty())
                            && !pending
                                .pending
                                .entries()
                                .any(|(_, _, position)| position.tier <= 1)
                        {
                            pending = queue
                                .work
                                .1
                                .wait_timeout(pending, std::time::Duration::from_millis(50))
                                .expect("preview queue lock poisoned")
                                .0;
                        }
                        pending.pending.pop().map(|(key, request, position)| {
                            let schedule_key = PreviewScheduleKey {
                                path: key.path.clone(),
                                level: key.level,
                            };
                            if let Some(keys) = pending.pending_keys.get_mut(&schedule_key) {
                                keys.remove(&key);
                                if keys.is_empty() {
                                    pending.pending_keys.remove(&schedule_key);
                                }
                            }
                            let request = Arc::new(Mutex::new(request));
                            pending.active.insert(key.clone(), request.clone());
                            (key, request, position)
                        })
                    };
                    let Some((key, request, position)) = request else {
                        continue;
                    };
                    let _foreground =
                        (position.tier <= 1).then(|| queue.library.foreground.enter());
                    let (path, preview_dir, kind, source_revision, level, valid_at, cancellation) = {
                        let request = request
                            .lock()
                            .expect("active preview request lock poisoned");
                        (
                            request.path.clone(),
                            request.preview_dir.clone(),
                            request.kind,
                            request.source_revision.clone(),
                            request.level,
                            request.valid_at,
                            request.cancellation.clone(),
                        )
                    };
                    let (result, completions) =
                        match oxy_media::preview_for_app_with_completion(
                            &path,
                            &preview_dir,
                            level,
                            priority_from_position(position),
                            kind,
                            &cancellation,
                        ) {
                            Ok(preview) => (Ok(preview.result), preview.completions),
                            Err(error) => (Err(error.to_string()), Vec::new()),
                        };
                    let projection = match &result {
                        Ok(result) => queue.transition(
                            path.clone(),
                            ProjectionUpdate {
                                source_revision: source_revision.clone(),
                                level,
                                valid_at,
                                status: ResourceLoadStatus::Ready,
                                result: Some(result.clone()),
                                error: None,
                            },
                        ),
                        Err(error) => queue.transition(
                            path.clone(),
                            ProjectionUpdate {
                                source_revision: source_revision.clone(),
                                level,
                                valid_at,
                                status: ResourceLoadStatus::Error,
                                result: None,
                                error: Some(error.clone()),
                            },
                        ),
                    };
                    let (projection, accepted) = match projection {
                        Ok(projection)
                            if projection.status == ResourceLoadStatus::Ready
                                && projection.result.is_some() =>
                        {
                            (Some(projection.clone()), Ok(projection))
                        }
                        Ok(projection) => (
                            Some(projection),
                            Err(result
                                .as_ref()
                                .err()
                                .cloned()
                                .unwrap_or_else(|| "image request was superseded".to_owned())),
                        ),
                        Err(error) => (None, Err(error)),
                    };
                    if let Some(projection) = projection
                        .filter(|projection| projection.error.as_deref() != Some("invalidated"))
                    {
                        let _ = app.emit(IMAGE_PROJECTION_UPDATED_EVENT, projection);
                    }
                    for completion in completions {
                        queue.spawn_persistence_completion(
                            app.clone(),
                            path.clone(),
                            source_revision.clone(),
                            level,
                            valid_at,
                            completion,
                        );
                    }
                    let is_interim = result.as_ref().is_ok_and(|result| {
                        result.satisfaction == Some(MediaSatisfaction::Interim)
                    });
                    if is_interim {
                        // Keep the request active after the displayable Interim
                        // reply. Its subscribers and cancellation token continue
                        // to own the no-Interim upgrade and its queue priority.
                        let mut active = request
                            .lock()
                            .expect("active preview request lock poisoned");
                        for waiter in &mut active.waiters {
                            if !waiter.interim_delivered {
                                let _ = waiter.sender.send(accepted.clone());
                                waiter.interim_delivered = true;
                            }
                        }
                        drop(active);
                    }
                    let final_accepted = if is_interim {
                        match queue.run_interim_upgrade(
                            &app,
                            &path,
                            &preview_dir,
                            kind,
                            &source_revision,
                            level,
                            valid_at,
                            priority_from_position(position),
                            &cancellation,
                        ) {
                            Ok(Some(projection)) => Ok(projection),
                            Ok(None) => accepted.clone(),
                            Err(error) => Err(error),
                        }
                    } else {
                        accepted.clone()
                    };
                    let waiters = {
                        let mut work = queue.work.0.lock().expect("preview queue lock poisoned");
                        let waiters = std::mem::take(
                            &mut request
                                .lock()
                                .expect("active preview request lock poisoned")
                                .waiters,
                        );
                        work.active.remove(&key);
                        for waiter in &waiters {
                            let changes = work.schedule.release_scope(&waiter.id);
                            work.apply_schedule_changes(changes);
                        }
                        waiters
                    };
                    for waiter in waiters {
                        if !waiter.interim_delivered {
                            let _ = waiter.sender.send(final_accepted.clone());
                        }
                    }
                    queue.work.1.notify_all();
                }
            })
            .expect("failed to start image projection worker");
    }
}

fn debug_item(
    key: &RequestKey,
    request: &WorkRequest,
    position: SchedulePosition,
) -> DebugQueueItem {
    let priority = priority_from_position(position);
    DebugQueueItem {
        key: format!("{}:{:?}:{}", key.path.display(), key.level, key.generation),
        path: Some(key.path.clone()),
        root_path: None,
        stage: format!("{:?}", key.level).to_lowercase(),
        priority: format!("{priority:?}").to_lowercase(),
        rank: Some(i64::from(position.rank)),
        consumers: request.waiters.len(),
        pending_count: None,
        asset_count: None,
        directory_count: None,
    }
}

fn projection_is_terminal(projection: &ImageProjection) -> bool {
    projection.status == ResourceLoadStatus::Ready
        && projection.result.as_ref().is_some_and(|result| {
            projection.level == RenderLevel::Thumbnail
                || result.satisfaction != Some(MediaSatisfaction::Interim)
        })
}

fn restore_projection_resource(
    mut projection: ImageProjection,
    preview_dir: &std::path::Path,
) -> Result<ImageProjection, String> {
    let expected_source =
        oxy_media::SourceRevision::observe(&projection.path).map_err(|error| error.to_string())?;
    if !projection
        .source_revision
        .starts_with(&format!("{}:", expected_source.revision_id))
    {
        return Ok(projection);
    }
    let Some(result) = projection.result.as_mut() else {
        return Ok(projection);
    };
    if result.resource.as_ref().is_some_and(|resource| {
        oxy_media::shared_resource_registry().refresh_publication_grace(&resource.resource_id)
    }) {
        return Ok(projection);
    }
    result.resource = None;
    if !result.path.is_file() {
        return Ok(projection);
    }
    let representation = match result.kind {
        PreviewKind::Original => oxy_media::ArtifactRepresentation::Original,
        PreviewKind::Embedded => oxy_media::ArtifactRepresentation::Embedded,
        PreviewKind::Decoded => oxy_media::ArtifactRepresentation::Decoded,
        PreviewKind::Developed => oxy_media::ArtifactRepresentation::Developed,
        PreviewKind::System => oxy_media::ArtifactRepresentation::System,
    };
    let disk_cache =
        oxy_media::DiskMediaCache::new(preview_dir, 256).map_err(|error| error.to_string())?;
    let lease = if result.path.starts_with(disk_cache.root()) {
        let Some(lease) = disk_cache
            .validate_and_lease_path(&result.path, &expected_source)
            .map_err(|error| error.to_string())?
        else {
            return Ok(projection);
        };
        Some(lease)
    } else {
        None
    };
    if lease.is_some() {
        // This request restored a validated disk artifact, not the decoder
        // invocation whose historical timings may be stored in the projection.
        result.diagnostics = Some(oxy_domain::PreviewDiagnostics {
            backend: Some("cached artifact".into()),
            ..Default::default()
        });
    }
    let handle = oxy_media::shared_resource_registry()
        .register_file_with_lease(
            &result.path,
            media_type_for(&result.path),
            oxy_media::DisplayDimensions {
                width: result.width,
                height: result.height,
            },
            representation,
            lease,
        )
        .map_err(|error| error.to_string())?;
    result.resource = Some(MediaResourceDescriptor {
        resource_id: handle.descriptor.resource_id,
        url: handle.descriptor.url,
        media_type: handle.descriptor.media_type,
    });
    Ok(projection)
}

fn media_type_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
}

fn preview_result_is_live(result: &PreviewResult) -> bool {
    result.resource.as_ref().is_some_and(|resource| {
        oxy_media::shared_resource_registry().contains(&resource.resource_id)
    })
}

fn source_revision(
    path: &std::path::Path,
    modified_at_ms: u64,
    size_bytes: u64,
    preview_dir: &std::path::Path,
    kind: AssetKind,
    level: RenderLevel,
) -> Result<String, String> {
    let media_revision =
        oxy_media::SourceRevision::observe(path).map_err(|error| error.to_string())?;
    Ok(format!(
        "{}:{modified_at_ms}:{size_bytes}:{}:{}",
        media_revision.revision_id,
        preview_dir.to_string_lossy(),
        oxy_media::preview_policy_revision(kind, level)
    ))
}

fn runtime_placement(placement: SchedulePlacement) -> QueuePlacement {
    match placement {
        SchedulePlacement::Front => QueuePlacement::Front,
        SchedulePlacement::Back => QueuePlacement::Back,
    }
}

fn omitted_policy(policy: PreviewOmittedPolicy) -> Result<OmittedIntentPolicy, String> {
    match policy.action {
        OmittedScheduleAction::Release => Ok(OmittedIntentPolicy::Release),
        OmittedScheduleAction::Demote => {
            let priority = policy
                .priority
                .ok_or_else(|| "demote policy requires priority".to_owned())?;
            let placement = policy
                .placement
                .ok_or_else(|| "demote policy requires placement".to_owned())?;
            Ok(OmittedIntentPolicy::Demote {
                tier: schedule_position(priority, 0).tier,
                placement: runtime_placement(placement),
            })
        }
    }
}

fn schedule_position(priority: PreviewPriority, queue_order: usize) -> SchedulePosition {
    let tier = match priority {
        PreviewPriority::Loupe => 0,
        PreviewPriority::Visible => 1,
        PreviewPriority::Nearby => 2,
        PreviewPriority::Preload => 3,
    };
    SchedulePosition::new(tier, u32::try_from(queue_order).unwrap_or(u32::MAX))
}

fn priority_from_position(position: SchedulePosition) -> PreviewPriority {
    match position.tier {
        0 => PreviewPriority::Loupe,
        1 => PreviewPriority::Visible,
        2 => PreviewPriority::Nearby,
        _ => PreviewPriority::Preload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_before_admission_is_consumed_and_bounded() {
        let mut state = WorkState::default();
        state.remember_early_cancellation("early");
        assert!(state.take_early_cancellation("early"));
        assert!(!state.take_early_cancellation("early"));
        for index in 0..2048 {
            state.remember_early_cancellation(&format!("request-{index}"));
        }
        assert_eq!(state.cancelled_before_admission.len(), 1024);
        assert!(!state.take_early_cancellation("request-0"));
        assert!(state.take_early_cancellation("request-2047"));
        state.cancelled_before_admission.clear();
        state
            .cancelled_before_admission
            .push_back(("expired".into(), Instant::now() - Duration::from_secs(121)));
        assert!(!state.take_early_cancellation("expired"));
    }

    #[test]
    fn interim_projection_is_displayable_but_not_terminal() {
        let projection = ImageProjection {
            path: PathBuf::from("image.HIF"),
            source_revision: "source".into(),
            projection_revision: 1,
            valid_at: 1,
            status: ResourceLoadStatus::Ready,
            level: RenderLevel::Preview,
            result: Some(PreviewResult {
                path: PathBuf::new(),
                width: 160,
                height: 120,
                kind: PreviewKind::Embedded,
                render_level: RenderLevel::Preview,
                resource: None,
                satisfaction: Some(MediaSatisfaction::Interim),
                persistence: Some(MediaPersistence::Pending),
                diagnostics: None,
            }),
            error: None,
        };
        assert!(!projection_is_terminal(&projection));
        let mut thumbnail = projection.clone();
        thumbnail.level = RenderLevel::Thumbnail;
        thumbnail.result.as_mut().unwrap().render_level = RenderLevel::Thumbnail;
        assert!(projection_is_terminal(&thumbnail));
        let mut satisfied = projection;
        satisfied.result.as_mut().unwrap().satisfaction = Some(MediaSatisfaction::Satisfied);
        assert!(projection_is_terminal(&satisfied));
    }

    #[test]
    fn failed_interim_upgrade_publishes_a_terminal_error_and_retains_display_result() {
        let state = tempfile::tempdir().unwrap();
        let library = Arc::new(Library::in_memory().unwrap());
        let cache = Arc::new(
            CacheManager::load(
                state.path().join("previews"),
                state.path().join("cache-config.json"),
            )
            .unwrap(),
        );
        let queue = PreviewQueue {
            work: Arc::new((Mutex::new(WorkState::default()), Condvar::new())),
            projections: Arc::new(RwLock::new(HashMap::new())),
            library,
            cache,
            request_generation: Arc::new(AtomicU64::new(0)),
        };
        let path = state.path().join("image.HIF");
        std::fs::write(&path, b"source").unwrap();
        let interim = PreviewResult {
            path: state.path().join("interim.jpg"),
            width: 160,
            height: 120,
            kind: PreviewKind::Embedded,
            render_level: RenderLevel::Preview,
            resource: None,
            satisfaction: Some(MediaSatisfaction::Interim),
            persistence: Some(MediaPersistence::Pending),
            diagnostics: None,
        };
        queue
            .transition(
                path.clone(),
                ProjectionUpdate {
                    source_revision: "source".into(),
                    level: RenderLevel::Preview,
                    valid_at: 1,
                    status: ResourceLoadStatus::Ready,
                    result: Some(interim.clone()),
                    error: None,
                },
            )
            .unwrap();

        let terminal = queue
            .transition_interim_upgrade_error(
                &path,
                "source",
                RenderLevel::Preview,
                1,
                "decoder failed".into(),
            )
            .unwrap();

        assert_eq!(terminal.status, ResourceLoadStatus::Error);
        assert_eq!(terminal.error.as_deref(), Some("decoder failed"));
        assert_eq!(terminal.result, Some(interim));
    }

    #[test]
    fn restored_managed_projection_reports_this_cache_hit_not_historical_decode() {
        use oxy_media::MediaCache;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.raw");
        std::fs::write(&path, b"source").unwrap();
        let source = oxy_media::SourceRevision::observe(&path).unwrap();
        let cache = oxy_media::DiskMediaCache::new(directory.path(), 8).unwrap();
        let png: &[u8] = &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 207,
            192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
            130,
        ];
        let publication = cache
            .publish(oxy_media::PendingArtifact {
                source_revision: source.clone(),
                variant: oxy_media::VariantIdentity {
                    representation: oxy_media::ArtifactRepresentation::Embedded,
                    presentation: oxy_media::ArtifactPresentation {
                        orientation: oxy_media::OrientationState::Applied,
                        color: oxy_media::CacheColorState::EmbeddedOrUnknown,
                        sharpening: oxy_media::SharpeningState::None,
                    },
                    policy_revision: oxy_media::MEDIA_CACHE_POLICY_REVISION,
                    target: "test".into(),
                },
                actual_dimensions: oxy_media::DisplayDimensions {
                    width: 1,
                    height: 1,
                },
                native_detail: false,
                media_type: "image/png".into(),
                extension: "png".into(),
                bytes: Arc::from(png),
                cache_generation: cache.generation().unwrap(),
            })
            .unwrap();
        let oxy_media::ArtifactLocation::Managed(managed_path) = publication.artifact.location
        else {
            panic!("expected managed artifact");
        };
        let projection = ImageProjection {
            path,
            source_revision: format!("{}:projection", source.revision_id),
            projection_revision: 1,
            valid_at: 1,
            status: ResourceLoadStatus::Ready,
            level: RenderLevel::Preview,
            result: Some(PreviewResult {
                path: managed_path,
                width: 1,
                height: 1,
                kind: PreviewKind::Embedded,
                render_level: RenderLevel::Preview,
                resource: None,
                satisfaction: Some(MediaSatisfaction::Interim),
                persistence: Some(MediaPersistence::Persisted),
                diagnostics: Some(oxy_domain::PreviewDiagnostics {
                    backend: Some("old decoder".into()),
                    decode_ms: Some(3000),
                    ..Default::default()
                }),
            }),
            error: None,
        };
        let restored = restore_projection_resource(projection, directory.path()).unwrap();
        let result = restored.result.unwrap();
        assert!(result.resource.is_some());
        let diagnostics = result.diagnostics.unwrap();
        assert_eq!(diagnostics.backend.as_deref(), Some("cached artifact"));
        assert_eq!(diagnostics.decode_ms, None);
    }

    #[test]
    fn restored_projection_registers_a_new_process_resource() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("original.jpg");
        std::fs::write(&path, b"original bytes").unwrap();
        let source = oxy_media::SourceRevision::observe(&path).unwrap();
        let projection = ImageProjection {
            path: path.clone(),
            source_revision: format!("{}:projection", source.revision_id),
            projection_revision: 1,
            valid_at: 1,
            status: ResourceLoadStatus::Ready,
            level: RenderLevel::Thumbnail,
            result: Some(PreviewResult {
                path,
                width: 8,
                height: 4,
                kind: PreviewKind::Original,
                render_level: RenderLevel::Thumbnail,
                resource: Some(MediaResourceDescriptor {
                    resource_id: "resource-from-old-process".into(),
                    url: "oxy-media://localhost/resource/resource-from-old-process".into(),
                    media_type: "image/jpeg".into(),
                }),
                satisfaction: Some(MediaSatisfaction::Satisfied),
                persistence: Some(MediaPersistence::NotApplicable),
                diagnostics: None,
            }),
            error: None,
        };
        let library = Arc::new(Library::in_memory().unwrap());
        let accepted = library.accept_image_projection(projection).unwrap();
        let queue = PreviewQueue {
            work: Arc::new((Mutex::new(WorkState::default()), Condvar::new())),
            projections: Arc::new(RwLock::new(HashMap::new())),
            library,
            cache: Arc::new(
                CacheManager::load(
                    directory.path().join("previews"),
                    directory.path().join("settings.json"),
                )
                .unwrap(),
            ),
            request_generation: Arc::new(AtomicU64::new(0)),
        };
        let restored = queue
            .restore_cached_projection(accepted.clone(), directory.path())
            .unwrap();
        assert!(restored.projection_revision > accepted.projection_revision);
        let resource = restored.result.as_ref().unwrap().resource.as_ref().unwrap();
        assert_ne!(resource.resource_id, "resource-from-old-process");
        let registry = oxy_media::shared_resource_registry();
        assert!(registry.contains(&resource.resource_id));
        registry.release(&resource.resource_id);
        // A subsequent publication sweeps the released entry, as happens when
        // a virtual row leaves and other images enter the viewport.
        let original = &restored.result.as_ref().unwrap().path;
        let _other = registry
            .register_file(
                original,
                "image/jpeg",
                oxy_media::DisplayDimensions {
                    width: 8,
                    height: 4,
                },
                oxy_media::ArtifactRepresentation::Original,
            )
            .unwrap();
        assert!(!registry.contains(&resource.resource_id));
        let replacement = queue
            .restore_cached_projection(restored.clone(), directory.path())
            .unwrap();
        assert!(replacement.projection_revision > restored.projection_revision);
        assert_ne!(
            replacement.result.as_ref().unwrap().resource,
            restored.result.as_ref().unwrap().resource
        );
        // Replaying an older disk snapshot must preserve this newer descriptor.
        let replay = queue
            .restore_cached_projection(accepted, directory.path())
            .unwrap();
        assert_eq!(replay.projection_revision, replacement.projection_revision);
        assert_eq!(
            replay.result.unwrap().resource,
            replacement.result.unwrap().resource
        );
    }

    #[test]
    fn projection_accepts_live_resource_but_rejects_missing_resource_and_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("original.jpg");
        std::fs::write(&path, b"registered resource").unwrap();
        let registry = oxy_media::shared_resource_registry();
        let handle = registry
            .register_file(
                &path,
                "image/jpeg",
                oxy_media::DisplayDimensions {
                    width: 8,
                    height: 4,
                },
                oxy_media::ArtifactRepresentation::Original,
            )
            .unwrap();
        let mut result = PreviewResult {
            path: directory.path().join("pending.jpg"),
            width: 8,
            height: 4,
            kind: oxy_domain::PreviewKind::Original,
            render_level: RenderLevel::Thumbnail,
            resource: Some(oxy_domain::MediaResourceDescriptor {
                resource_id: handle.descriptor.resource_id,
                url: handle.descriptor.url,
                media_type: "image/jpeg".into(),
            }),
            satisfaction: Some(oxy_domain::MediaSatisfaction::Satisfied),
            persistence: Some(oxy_domain::MediaPersistence::Pending),
            diagnostics: None,
        };
        assert!(preview_result_is_live(&result));
        registry.release(&result.resource.as_ref().unwrap().resource_id);
        result.resource.as_mut().unwrap().resource_id = "missing-resource".into();
        assert!(!preview_result_is_live(&result));
    }

    #[test]
    fn source_revision_includes_strong_media_identity_and_policy_version() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.hif");
        std::fs::write(&path, b"source revision").unwrap();
        let media_revision = oxy_media::SourceRevision::observe(&path).unwrap();
        let revision = source_revision(
            &path,
            12,
            34,
            std::path::Path::new("cache"),
            AssetKind::Heif,
            RenderLevel::Full,
        )
        .unwrap();
        assert!(revision.starts_with(&media_revision.revision_id));
        assert!(revision.ends_with(oxy_media::preview_policy_revision(
            AssetKind::Heif,
            RenderLevel::Full,
        )));
    }

    #[test]
    fn interim_upgrade_stays_cancellable_after_its_display_reply() {
        let (sender, _receiver) = mpsc::channel();
        let cancellation = CancellationToken::default();
        let mut request = WorkRequest {
            path: PathBuf::from("image.HIF"),
            preview_dir: PathBuf::from("cache"),
            kind: AssetKind::Heif,
            source_revision: "source".into(),
            level: RenderLevel::Preview,
            valid_at: 1,
            waiters: vec![Waiter {
                id: "consumer".into(),
                sender,
                interim_delivered: true,
            }],
            cancellation: cancellation.clone(),
        };

        assert!(request.remove_waiter("consumer"));
        assert!(cancellation.is_cancelled());
    }

    #[test]
    fn selected_order_is_preserved_inside_each_priority_tier() {
        assert!(
            schedule_position(PreviewPriority::Loupe, 999_999)
                > schedule_position(PreviewPriority::Visible, 0)
        );
        assert!(
            schedule_position(PreviewPriority::Visible, 0)
                > schedule_position(PreviewPriority::Visible, 10)
        );
        assert_eq!(
            priority_from_position(schedule_position(PreviewPriority::Nearby, 4)),
            PreviewPriority::Nearby
        );
    }
}
