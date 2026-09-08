use oxy_domain::{
    AssetKind, DebugQueueItem, DebugQueueState, ImageProjection, OmittedScheduleAction,
    PreviewOmittedPolicy, PreviewPriority, PreviewResult, PreviewScheduleIntent, RenderLevel,
    ResourceLoadStatus, SchedulePlacement,
};
use oxy_library::Library;
use oxy_runtime::{
    CancellationToken, CoalescingPriorityQueue, EffectiveScheduleChange, OmittedIntentPolicy,
    QueuePlacement, SchedulePosition, ScopedIntentScheduler,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
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

struct Waiter {
    id: String,
    sender: Sender<Result<ImageProjection, String>>,
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
}

impl WorkState {
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
    request_generation: Arc<AtomicU64>,
}

impl PreviewQueue {
    pub fn new(app: AppHandle, library: Arc<Library>) -> Self {
        let queue = Self {
            work: Arc::new((Mutex::new(WorkState::default()), Condvar::new())),
            projections: Arc::new(RwLock::new(HashMap::new())),
            library,
            request_generation: Arc::new(AtomicU64::new(0)),
        };
        for worker_index in 0..PREVIEW_WORKER_COUNT {
            queue.spawn_worker(app.clone(), worker_index);
        }
        queue
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
            source_revision(modified_at_ms, size_bytes, &preview_dir, kind, level);
        let state_key = (path.clone(), level);
        if let Some(cached) = self
            .projections
            .read()
            .expect("image projection lock poisoned")
            .get(&state_key)
            .filter(|projection| {
                projection.source_revision == source_revision
                    && projection.status == ResourceLoadStatus::Ready
                    && projection
                        .result
                        .as_ref()
                        .is_some_and(|result| result.path.is_file())
            })
            .cloned()
        {
            let (sender, receiver) = mpsc::channel();
            let _ = sender.send(Ok(cached.clone()));
            return Ok((cached, receiver));
        }
        if let Some(cached) = self
            .library
            .image_projection(&path, level, &source_revision)
            .map_err(|error| error.to_string())?
            .filter(|projection| {
                projection.status == ResourceLoadStatus::Ready
                    && projection
                        .result
                        .as_ref()
                        .is_some_and(|result| result.path.is_file())
            })
        {
            self.projections
                .write()
                .expect("image projection lock poisoned")
                .insert(state_key, cached.clone());
            let (sender, receiver) = mpsc::channel();
            let _ = sender.send(Ok(cached.clone()));
            return Ok((cached, receiver));
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
            let previous_len = request.waiters.len();
            request.waiters.retain(|waiter| waiter.id != request_id);
            changed |= request.waiters.len() != previous_len;
            if request.waiters.is_empty() {
                request.cancellation.cancel();
            }
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
                    let result = oxy_media::preview(
                        &path,
                        &preview_dir,
                        level,
                        priority_from_position(position),
                        kind,
                        &cancellation,
                    )
                    .map_err(|error| error.to_string());
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
                            path,
                            ProjectionUpdate {
                                source_revision,
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
                        let _ = waiter.sender.send(accepted.clone());
                    }
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

fn source_revision(
    modified_at_ms: u64,
    size_bytes: u64,
    preview_dir: &std::path::Path,
    kind: AssetKind,
    level: RenderLevel,
) -> String {
    format!(
        "{modified_at_ms}:{size_bytes}:{}:{}",
        preview_dir.to_string_lossy(),
        oxy_media::preview_policy_revision(kind, level)
    )
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
    fn source_revision_includes_media_policy_version() {
        let revision = source_revision(
            12,
            34,
            std::path::Path::new("cache"),
            AssetKind::Heif,
            RenderLevel::Full,
        );
        assert!(revision.ends_with(oxy_media::preview_policy_revision(
            AssetKind::Heif,
            RenderLevel::Full,
        )));
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
