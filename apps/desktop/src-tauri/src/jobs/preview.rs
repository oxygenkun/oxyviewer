use oxy_domain::{
    AssetKind, ImageProjection, PreviewPriority, PreviewResult, RenderLevel, ResourceLoadStatus,
};
use oxy_library::Library;
use oxy_runtime::CoalescingPriorityQueue;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
};
use tauri::{AppHandle, Emitter};

pub const IMAGE_PROJECTION_UPDATED_EVENT: &str = "image-projection-updated";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RequestKey {
    path: PathBuf,
    source_revision: String,
    level: RenderLevel,
    generation: u64,
}

struct WorkRequest {
    path: PathBuf,
    preview_dir: PathBuf,
    kind: AssetKind,
    source_revision: String,
    level: RenderLevel,
    valid_at: u64,
    waiters: Vec<Sender<Result<ImageProjection, String>>>,
}

pub struct PreviewRequest {
    pub path: PathBuf,
    pub preview_dir: PathBuf,
    pub kind: AssetKind,
    pub size_bytes: u64,
    pub modified_at_ms: u64,
    pub level: RenderLevel,
    pub priority: PreviewPriority,
    pub queue_order: usize,
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
    pending: CoalescingPriorityQueue<RequestKey, WorkRequest, i64>,
    active: HashMap<RequestKey, Arc<Mutex<WorkRequest>>>,
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
        queue.spawn_worker(app);
        queue
    }

    pub fn request(
        &self,
        app: &AppHandle,
        request: PreviewRequest,
    ) -> Result<(ImageProjection, Receiver<Result<ImageProjection, String>>), String> {
        let PreviewRequest {
            path,
            preview_dir,
            kind,
            size_bytes,
            modified_at_ms,
            level,
            priority,
            queue_order,
        } = request;
        let source_revision = format!(
            "{modified_at_ms}:{size_bytes}:{}",
            preview_dir.to_string_lossy()
        );
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
        let mut work = self.work.0.lock().expect("preview queue lock poisoned");
        if let Some(active) = work.active.get(&key) {
            active
                .lock()
                .expect("active preview request lock poisoned")
                .waiters
                .push(sender);
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
            let updated = work.pending.update_if_present(
                &key,
                priority_score(priority, queue_order),
                |current| current.waiters.push(sender),
            );
            debug_assert!(updated);
            return Ok((projection, receiver));
        }

        let valid_at = self
            .library
            .next_resource_revision()
            .map_err(|error| error.to_string())?;
        let loading = self.transition(
            path.clone(),
            ProjectionUpdate {
                source_revision: source_revision.clone(),
                level,
                valid_at,
                status: ResourceLoadStatus::Loading,
                result: None,
                error: None,
            },
        )?;
        let _ = app.emit(IMAGE_PROJECTION_UPDATED_EVENT, loading.clone());
        let request = WorkRequest {
            path,
            preview_dir,
            kind,
            source_revision,
            level,
            valid_at,
            waiters: vec![sender],
        };
        work.pending.push_or_merge(
            key,
            request,
            priority_score(priority, queue_order),
            |current, replacement| {
                current.waiters.extend(replacement.waiters);
            },
        );
        drop(work);
        self.work.1.notify_one();
        Ok((loading, receiver))
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

    fn spawn_worker(&self, app: AppHandle) {
        let queue = self.clone();
        std::thread::Builder::new()
            .name("oxy-image-projection".into())
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
                        pending.pending.pop().map(|(key, request, score)| {
                            let request = Arc::new(Mutex::new(request));
                            pending.active.insert(key.clone(), request.clone());
                            (key, request, score)
                        })
                    };
                    let Some((key, request, score)) = request else {
                        continue;
                    };
                    let (path, preview_dir, kind, source_revision, level, valid_at) = {
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
                        )
                    };
                    let result = oxy_media::preview(
                        &path,
                        &preview_dir,
                        level,
                        oxy_media::decode_priority_for(priority_from_score(score)),
                        kind,
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
                        waiters
                    };
                    for waiter in waiters {
                        let _ = waiter.send(accepted.clone());
                    }
                }
            })
            .expect("failed to start image projection worker");
    }
}

fn priority_score(priority: PreviewPriority, queue_order: usize) -> i64 {
    let tier = match priority {
        PreviewPriority::Preload => 0,
        PreviewPriority::Nearby => 1,
        PreviewPriority::Visible => 2,
        PreviewPriority::Loupe => 3,
    };
    i64::from(tier) * 1_000_000 - i64::try_from(queue_order.min(999_999)).unwrap_or_default()
}

fn priority_from_score(score: i64) -> PreviewPriority {
    if score > 2_000_000 {
        PreviewPriority::Loupe
    } else if score > 1_000_000 {
        PreviewPriority::Visible
    } else if score > 0 {
        PreviewPriority::Nearby
    } else {
        PreviewPriority::Preload
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_order_is_preserved_inside_each_priority_tier() {
        assert!(
            priority_score(PreviewPriority::Loupe, 999_999)
                > priority_score(PreviewPriority::Visible, 0)
        );
        assert!(
            priority_score(PreviewPriority::Visible, 0)
                > priority_score(PreviewPriority::Visible, 10)
        );
        assert_eq!(
            priority_from_score(priority_score(PreviewPriority::Nearby, 4)),
            PreviewPriority::Nearby
        );
    }
}
