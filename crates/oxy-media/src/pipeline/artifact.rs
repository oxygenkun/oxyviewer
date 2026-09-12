//! Helpers shared by preview artifact pipelines.

use crate::{
    MediaError,
    cache::{
        ArtifactPresentation, ArtifactRequirement, CacheColorState, CacheRequest,
        DetailRequirement, DiskMediaCache, ImageOrigin, MEDIA_CACHE_POLICY_REVISION, MediaCache,
        OrientationRequirement, OrientationState, PendingArtifact, PixelDimensions,
        PresentationRequirement, Satisfaction, SharpeningState, SourceRevision, VariantIdentity,
    },
};
#[cfg(test)]
use oxy_domain::PreviewKind;
use oxy_domain::{PreviewResult, RenderLevel};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    ops::Deref,
    path::Path,
    sync::{Arc, Condvar, Mutex, OnceLock, mpsc::Receiver},
    time::{Duration, Instant},
};

thread_local! {
    static APP_PUBLICATION: Cell<bool> = const { Cell::new(false) };
    static APP_COMPLETIONS: RefCell<Vec<Receiver<crate::publication::PersistenceCompletion>>> =
        const { RefCell::new(Vec::new()) };
}

const MAX_RUNNING_WORK_RECORDS: usize = 64;
static RUNNING_WORK: OnceLock<Mutex<VecDeque<Arc<RunningWork>>>> = OnceLock::new();

struct RunningWork {
    key: String,
    promise: CacheRequest,
    cache_generation: u64,
    state: Mutex<RunningWorkState>,
    ready: Condvar,
}

enum RunningWorkState {
    Pending,
    Ready(Box<PreviewResult>),
    Failed,
}

enum WorkRole {
    Producer(Arc<RunningWork>),
    Waiter(Arc<RunningWork>),
    Uncoordinated,
}

struct RunningWorkProducerGuard {
    work: Arc<RunningWork>,
    finished: bool,
}

impl RunningWorkProducerGuard {
    fn new(work: Arc<RunningWork>) -> Self {
        Self {
            work,
            finished: false,
        }
    }

    fn finish(&mut self, outcome: &Result<PreviewResult, MediaError>) {
        let mut state = self
            .work
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = match outcome {
            Ok(result) => RunningWorkState::Ready(Box::new(result.clone())),
            Err(_) => RunningWorkState::Failed,
        };
        self.finished = true;
        self.work.ready.notify_all();
    }
}

impl Drop for RunningWorkProducerGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        *self
            .work
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = RunningWorkState::Failed;
        self.work.ready.notify_all();
    }
}

pub(crate) fn with_app_publication<T>(
    operation: impl FnOnce() -> T,
) -> (T, Vec<Receiver<crate::publication::PersistenceCompletion>>) {
    APP_PUBLICATION.with(|mode| {
        let previous = mode.replace(true);
        APP_COMPLETIONS.with(|completions| completions.borrow_mut().clear());
        let result = operation();
        let completions =
            APP_COMPLETIONS.with(|completions| std::mem::take(&mut *completions.borrow_mut()));
        mode.set(previous);
        (result, completions)
    })
}

fn record_completion(completion: Option<Receiver<crate::publication::PersistenceCompletion>>) {
    if let Some(completion) = completion {
        APP_COMPLETIONS.with(|completions| completions.borrow_mut().push(completion));
    }
}

pub(crate) fn register_original_resource(
    path: &Path,
    mut result: PreviewResult,
) -> Result<PreviewResult, MediaError> {
    if !APP_PUBLICATION.with(Cell::get) {
        return Ok(result);
    }
    let media_type = match path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };
    let resource = crate::publication::shared_resource_registry().register_file(
        path,
        media_type,
        PixelDimensions {
            width: result.width,
            height: result.height,
        },
        ImageOrigin::PrimaryImage,
    )?;
    result.resource = Some(oxy_domain::MediaResourceDescriptor {
        resource_id: resource.descriptor.resource_id,
        url: resource.descriptor.url,
        media_type: resource.descriptor.media_type,
    });
    result.satisfaction = Some(oxy_domain::MediaSatisfaction::Satisfied);
    result.persistence = Some(oxy_domain::MediaPersistence::NotApplicable);
    Ok(result)
}

pub(crate) struct BackendOutput {
    path: tempfile::TempPath,
    _lease: crate::cache::ArtifactLease,
}

impl BackendOutput {
    fn into_parts(self) -> (tempfile::TempPath, crate::cache::ArtifactLease) {
        (self.path, self._lease)
    }
}

impl Deref for BackendOutput {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<Path> for BackendOutput {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

pub(crate) enum ArtifactPreparation {
    Cached(Box<PreviewResult>),
    Generate { cache_generation: u64 },
}

pub(crate) struct ArtifactCache {
    cache: DiskMediaCache,
    source: SourceRevision,
    async_publication: bool,
    publisher: Option<Arc<crate::publication::ArtifactPublisher>>,
}

#[derive(Clone, Copy)]
pub(crate) enum ArtifactEncoding {
    Jpeg,
    Png,
}

impl ArtifactEncoding {
    fn media_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
        }
    }
}

impl ArtifactCache {
    pub(crate) fn new(path: &Path, cache_dir: &Path) -> Result<Self, MediaError> {
        Self::for_source_revision(SourceRevision::observe(path)?, cache_dir)
    }

    pub(crate) fn for_source_revision(
        source: SourceRevision,
        cache_dir: &Path,
    ) -> Result<Self, MediaError> {
        if SourceRevision::observe(&source.canonical_path)? != source {
            return Err(MediaError::StaleSourceRevision);
        }
        let async_publication = APP_PUBLICATION.with(Cell::get);
        let cache = DiskMediaCache::new(cache_dir, 256)?;
        let publisher = async_publication
            .then(|| {
                crate::publication::shared_publisher(cache.root().parent().unwrap_or(cache.root()))
            })
            .transpose()?;
        Ok(Self {
            cache,
            source,
            async_publication,
            publisher,
        })
    }

    pub(crate) fn source_revision_id(&self) -> &str {
        &self.source.revision_id
    }

    pub(crate) fn source_lock_key(&self, lane: &str) -> String {
        format!("{}:{lane}", self.source.revision_id)
    }

    /// Shares a bounded decoder-running promise and its produced UI result.
    /// The handoff does not depend on persistence reservation or queue state.
    pub(crate) fn coordinate_work(
        &self,
        request: &CacheRequest,
        level: RenderLevel,
        lane: &str,
        cancelled: impl Fn() -> bool,
        produce: impl FnOnce(u64) -> Result<PreviewResult, MediaError>,
    ) -> Result<PreviewResult, MediaError> {
        if cancelled() {
            return Err(MediaError::Cancelled);
        }
        let cache_generation = match self.prepare(request, level)? {
            ArtifactPreparation::Cached(result) => return Ok(*result),
            ArtifactPreparation::Generate { cache_generation } => cache_generation,
        };
        let lock_key = self.source_lock_key(lane);
        let work_key = format!("{}:{}", lock_key, self.cache.root().display());
        match running_work_role(&work_key, request, cache_generation) {
            WorkRole::Waiter(work) => {
                let mut state = work
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                loop {
                    match &*state {
                        RunningWorkState::Ready(result)
                            if result.satisfaction
                                != Some(oxy_domain::MediaSatisfaction::Interim) =>
                        {
                            let mut handoff = (**result).clone();
                            handoff.render_level = level;
                            handoff.satisfaction = Some(oxy_domain::MediaSatisfaction::Satisfied);
                            drop(state);
                            // A staged/encoded result is displayable before its
                            // persistence finishes. Re-enter the normal lookup
                            // path so every app waiter subscribes to that
                            // completion (or observes the committed artifact).
                            // Only backpressure-skipped output has neither.
                            return match self.prepare(request, level)? {
                                ArtifactPreparation::Cached(result) => Ok(*result),
                                ArtifactPreparation::Generate {
                                    cache_generation: _,
                                } if handoff.persistence
                                    == Some(oxy_domain::MediaPersistence::Skipped) =>
                                {
                                    Ok(handoff)
                                }
                                ArtifactPreparation::Generate { cache_generation } => {
                                    produce(cache_generation)
                                }
                            };
                        }
                        RunningWorkState::Ready(_) | RunningWorkState::Failed => break,
                        RunningWorkState::Pending => {
                            if cancelled() {
                                return Err(MediaError::Cancelled);
                            }
                            state = work
                                .ready
                                .wait_timeout(state, Duration::from_millis(10))
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .0;
                        }
                    }
                }
                drop(state);
                let lock = crate::decode_control::file_lock(&lock_key);
                let _guard = crate::decode_control::acquire_file_lock(&lock, &cancelled)?;
                match self.prepare(request, level)? {
                    ArtifactPreparation::Cached(result) => Ok(*result),
                    ArtifactPreparation::Generate { cache_generation } => produce(cache_generation),
                }
            }
            WorkRole::Uncoordinated => {
                let lock = crate::decode_control::file_lock(&lock_key);
                let _guard = crate::decode_control::acquire_file_lock(&lock, &cancelled)?;
                match self.prepare(request, level)? {
                    ArtifactPreparation::Cached(result) => Ok(*result),
                    ArtifactPreparation::Generate { cache_generation } => produce(cache_generation),
                }
            }
            WorkRole::Producer(work) => {
                let mut completion = RunningWorkProducerGuard::new(work);
                let lock = crate::decode_control::file_lock(&lock_key);
                let outcome = (|| {
                    let _guard = crate::decode_control::acquire_file_lock(&lock, &cancelled)?;
                    if cancelled() {
                        return Err(MediaError::Cancelled);
                    }
                    match self.prepare(request, level)? {
                        ArtifactPreparation::Cached(result) => Ok(*result),
                        ArtifactPreparation::Generate { cache_generation } => {
                            produce(cache_generation)
                        }
                    }
                })();
                completion.finish(&outcome);
                outcome
            }
        }
    }

    pub(crate) fn temporary_output(&self, suffix: &str) -> Result<BackendOutput, MediaError> {
        let directory = self.cache.root().join(".backend-tmp");
        match std::fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(MediaError::CacheArtifact(
                    "backend staging path is not an owned directory".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&directory)?;
            }
            Err(error) => return Err(error.into()),
        }
        let temporary = tempfile::Builder::new()
            .suffix(suffix)
            .tempfile_in(directory)?
            .into_temp_path();
        let lease = self.cache.lease_path(&temporary)?;
        std::fs::remove_file(&temporary)?;
        Ok(BackendOutput {
            path: temporary,
            _lease: lease,
        })
    }

    pub(crate) fn request(
        &self,
        detail: DetailRequirement,
        artifact: ArtifactRequirement,
        presentation: PresentationRequirement,
        allow_interim: bool,
    ) -> CacheRequest {
        CacheRequest {
            source_revision: self.source.clone(),
            detail,
            artifact,
            presentation,
            policy_revision: MEDIA_CACHE_POLICY_REVISION,
            allow_interim,
        }
    }

    pub(crate) fn lookup(
        &self,
        request: &CacheRequest,
        level: RenderLevel,
    ) -> Result<Option<PreviewResult>, MediaError> {
        if self.async_publication
            && let Some(result) = self.lookup_active(request, level)?
        {
            return Ok(Some(result));
        }
        self.cache
            .lookup(request)?
            .map(|hit| self.preview_from_hit(hit, level))
            .transpose()
    }

    /// Performs the common final cache recheck and captures the generation
    /// fence immediately before format-specific production starts.
    pub(crate) fn prepare(
        &self,
        request: &CacheRequest,
        level: RenderLevel,
    ) -> Result<ArtifactPreparation, MediaError> {
        self.prepare_candidates(request, &[], level)
    }

    /// Inspect one manifest for ordered representations and keep the same
    /// generation fence on a miss. Call again after acquiring a producer lock.
    pub(crate) fn prepare_candidates(
        &self,
        request: &CacheRequest,
        alternatives: &[CacheRequest],
        level: RenderLevel,
    ) -> Result<ArtifactPreparation, MediaError> {
        if self.async_publication
            && let Some(result) = self.lookup_active(request, level)?
        {
            return Ok(ArtifactPreparation::Cached(Box::new(result)));
        }
        match self.cache.lookup_candidates(request, alternatives)? {
            crate::cache::CacheLookup::Hit(hit) => Ok(ArtifactPreparation::Cached(Box::new(
                self.preview_from_hit(*hit, level)?,
            ))),
            crate::cache::CacheLookup::Generate { cache_generation } => {
                if self.async_publication {
                    for candidate in alternatives {
                        if let Some(result) = self.lookup_active(candidate, level)? {
                            return Ok(ArtifactPreparation::Cached(Box::new(result)));
                        }
                    }
                }
                Ok(ArtifactPreparation::Generate { cache_generation })
            }
        }
    }

    fn lookup_active(
        &self,
        request: &CacheRequest,
        level: RenderLevel,
    ) -> Result<Option<PreviewResult>, MediaError> {
        let Some(hit) = self
            .publisher
            .as_ref()
            .expect("app publication cache has a publisher")
            .lookup_active(request)?
        else {
            return Ok(None);
        };
        record_completion(hit.completion);
        let crate::cache::ArtifactLocation::Managed(path) = hit.artifact.location else {
            unreachable!("active publication uses a managed or staged file fact")
        };
        Ok(Some(PreviewResult {
            image_facts: Some(hit.artifact.facts.clone()),
            geometry: hit.artifact.variant.presentation.geometry,
            path,
            width: hit.artifact.facts.display_dimensions.0.width,
            height: hit.artifact.facts.display_dimensions.0.height,
            kind: hit.artifact.facts.preview_kind(),
            render_level: level,
            resource: Some(oxy_domain::MediaResourceDescriptor {
                resource_id: hit.resource.resource_id,
                url: hit.resource.url,
                media_type: hit.resource.media_type,
            }),
            satisfaction: Some(domain_satisfaction(hit.satisfaction)),
            persistence: Some(match hit.persistence {
                crate::publication::PersistenceStatus::NotRequested => {
                    oxy_domain::MediaPersistence::NotApplicable
                }
                crate::publication::PersistenceStatus::Scheduled => {
                    oxy_domain::MediaPersistence::Pending
                }
                crate::publication::PersistenceStatus::Persisted => {
                    oxy_domain::MediaPersistence::Persisted
                }
                crate::publication::PersistenceStatus::SkippedBackpressure => {
                    oxy_domain::MediaPersistence::Skipped
                }
            }),
            diagnostics: None,
        }))
    }

    fn preview_from_hit(
        &self,
        hit: crate::cache::CacheHit,
        level: RenderLevel,
    ) -> Result<PreviewResult, MediaError> {
        let crate::cache::CacheHit {
            artifact,
            satisfaction,
            lease,
        } = hit;
        let mut result = preview_result(artifact, satisfaction, level);
        if self.async_publication {
            let resource = crate::publication::shared_resource_registry()
                .register_file_with_lease(
                    &result.path,
                    "image/jpeg",
                    PixelDimensions {
                        width: result.width,
                        height: result.height,
                    },
                    result
                        .image_facts
                        .as_ref()
                        .expect("cached artifact has facts")
                        .source
                        .origin,
                    Some(lease),
                )?;
            result.resource = Some(oxy_domain::MediaResourceDescriptor {
                resource_id: resource.descriptor.resource_id,
                url: resource.descriptor.url,
                media_type: resource.descriptor.media_type,
            });
        }
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish(
        &self,
        bytes: Arc<[u8]>,
        facts: oxy_domain::ArtifactFacts,
        presentation: ArtifactPresentation,
        target: String,
        level: RenderLevel,
        generation: u64,
        request: &CacheRequest,
    ) -> Result<PreviewResult, MediaError> {
        self.publish_encoded(
            bytes,
            facts,
            presentation,
            target,
            level,
            generation,
            request,
            ArtifactEncoding::Jpeg,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish_encoded(
        &self,
        bytes: Arc<[u8]>,
        facts: oxy_domain::ArtifactFacts,
        presentation: ArtifactPresentation,
        target: String,
        level: RenderLevel,
        generation: u64,
        request: &CacheRequest,
        encoding: ArtifactEncoding,
    ) -> Result<PreviewResult, MediaError> {
        let mut facts = facts;
        crate::media_source::bind_facts(&mut facts, &self.source)?;
        let dimensions = facts.display_dimensions.0;
        let observed = SourceRevision::observe(&self.source.canonical_path)?;
        if observed != self.source {
            return Err(MediaError::StaleSourceRevision);
        }
        let pending = PendingArtifact {
            source_revision: self.source.clone(),
            variant: VariantIdentity {
                presentation,
                policy_revision: request.policy_revision,
                target,
            },
            facts: facts.clone(),
            media_type: encoding.media_type().into(),
            extension: encoding.extension().into(),
            bytes,
            cache_generation: generation,
        };
        let candidate = crate::cache::MediaArtifact {
            artifact_id: String::new(),
            source_revision: pending.source_revision.clone(),
            variant: pending.variant.clone(),
            facts: pending.facts.clone(),
            byte_size: pending.bytes.len() as u64,
            media_type: pending.media_type.clone(),
            location: crate::cache::ArtifactLocation::Managed(std::path::PathBuf::new()),
        };
        let satisfaction = crate::cache::satisfies(&candidate, request).ok_or_else(|| {
            MediaError::CacheArtifact("produced artifact is incompatible with its request".into())
        })?;
        if self.async_publication {
            let published = self
                .publisher
                .as_ref()
                .expect("app publication cache has a publisher")
                .publish(
                    crate::publication::ProducedArtifact {
                        source_revision: pending.source_revision.clone(),
                        variant: pending.variant.clone(),
                        facts: pending.facts.clone(),
                        cache_generation: pending.cache_generation,
                        payload: crate::publication::ProducedPayload::Encoded {
                            bytes: Arc::clone(&pending.bytes),
                            media_type: pending.media_type.clone(),
                            extension: pending.extension.clone(),
                        },
                    },
                    true,
                )?;
            let resource = oxy_domain::MediaResourceDescriptor {
                resource_id: published.resource.descriptor.resource_id.clone(),
                url: published.resource.descriptor.url.clone(),
                media_type: published.resource.descriptor.media_type.clone(),
            };
            record_completion(published.completion);
            return Ok(PreviewResult {
                image_facts: Some(facts.clone()),
                geometry: presentation.geometry,
                path: published.managed_path.unwrap_or_default(),
                width: dimensions.width,
                height: dimensions.height,
                kind: facts.preview_kind(),
                render_level: level,
                resource: Some(resource),
                satisfaction: Some(domain_satisfaction(satisfaction)),
                persistence: Some(match published.persistence {
                    crate::publication::PersistenceStatus::Scheduled => {
                        oxy_domain::MediaPersistence::Pending
                    }
                    crate::publication::PersistenceStatus::NotRequested => {
                        oxy_domain::MediaPersistence::NotApplicable
                    }
                    crate::publication::PersistenceStatus::Persisted => {
                        oxy_domain::MediaPersistence::Persisted
                    }
                    crate::publication::PersistenceStatus::SkippedBackpressure => {
                        oxy_domain::MediaPersistence::Skipped
                    }
                }),
                diagnostics: None,
            });
        }
        let publication = self.cache.publish(pending)?;
        Ok(preview_result(publication.artifact, satisfaction, level))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish_staged(
        &self,
        staged: BackendOutput,
        facts: oxy_domain::ArtifactFacts,
        presentation: ArtifactPresentation,
        target: String,
        level: RenderLevel,
        generation: u64,
        request: &CacheRequest,
    ) -> Result<PreviewResult, MediaError> {
        let mut facts = facts;
        crate::media_source::bind_facts(&mut facts, &self.source)?;
        let dimensions = facts.display_dimensions.0;
        let observed = SourceRevision::observe(&self.source.canonical_path)?;
        if observed != self.source {
            return Err(MediaError::StaleSourceRevision);
        }
        let (staged, _backend_lease) = staged.into_parts();
        if presentation.color == CacheColorState::Srgb {
            crate::cache::ensure_srgb_icc(&staged)?;
        }
        let variant = VariantIdentity {
            presentation,
            policy_revision: request.policy_revision,
            target,
        };
        let candidate = crate::cache::MediaArtifact {
            artifact_id: String::new(),
            source_revision: self.source.clone(),
            variant: variant.clone(),
            facts: facts.clone(),
            byte_size: std::fs::metadata(&staged)?.len(),
            media_type: "image/jpeg".into(),
            location: crate::cache::ArtifactLocation::Managed(staged.to_path_buf()),
        };
        let satisfaction = crate::cache::satisfies(&candidate, request).ok_or_else(|| {
            MediaError::CacheArtifact("produced artifact is incompatible with its request".into())
        })?;
        if self.async_publication {
            let publisher = self
                .publisher
                .as_ref()
                .expect("app publication cache has a publisher");
            // Move the completed native output outside the cache-owned v2 tree
            // before registration. Cache clear may remove backend staging, but
            // the UI resource remains stable while persistence copies it.
            let staged_owner = publisher.adopt_staged_file(staged, "jpg")?;
            let staged_path = staged_owner.path().to_owned();
            let published = publisher.publish(
                crate::publication::ProducedArtifact {
                    source_revision: self.source.clone(),
                    variant,
                    facts: facts.clone(),
                    cache_generation: generation,
                    payload: crate::publication::ProducedPayload::StagedFile {
                        owner: staged_owner,
                        media_type: "image/jpeg".into(),
                        extension: "jpg".into(),
                    },
                },
                true,
            )?;
            let resource = oxy_domain::MediaResourceDescriptor {
                resource_id: published.resource.descriptor.resource_id.clone(),
                url: published.resource.descriptor.url.clone(),
                media_type: published.resource.descriptor.media_type.clone(),
            };
            record_completion(published.completion);
            return Ok(PreviewResult {
                image_facts: Some(facts.clone()),
                geometry: presentation.geometry,
                path: staged_path,
                width: dimensions.width,
                height: dimensions.height,
                kind: facts.preview_kind(),
                render_level: level,
                resource: Some(resource),
                satisfaction: Some(domain_satisfaction(satisfaction)),
                persistence: Some(match published.persistence {
                    crate::publication::PersistenceStatus::Scheduled => {
                        oxy_domain::MediaPersistence::Pending
                    }
                    crate::publication::PersistenceStatus::NotRequested => {
                        oxy_domain::MediaPersistence::NotApplicable
                    }
                    crate::publication::PersistenceStatus::Persisted => {
                        oxy_domain::MediaPersistence::Persisted
                    }
                    crate::publication::PersistenceStatus::SkippedBackpressure => {
                        oxy_domain::MediaPersistence::Skipped
                    }
                }),
                diagnostics: None,
            });
        }
        let staged_path = staged.keep().map_err(|error| MediaError::Io(error.error))?;
        let publication = self
            .cache
            .publish_staged(crate::cache::PendingStagedArtifact {
                source_revision: self.source.clone(),
                variant,
                facts,
                media_type: "image/jpeg".into(),
                extension: "jpg".into(),
                staged_path,
                cache_generation: generation,
            })?;
        Ok(preview_result(publication.artifact, satisfaction, level))
    }
}

fn running_work_role(key: &str, request: &CacheRequest, cache_generation: u64) -> WorkRole {
    let records = RUNNING_WORK.get_or_init(|| Mutex::new(VecDeque::new()));
    let mut records = records
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    records.retain(|record| {
        if record.key != key || record.cache_generation == cache_generation {
            return true;
        }
        matches!(
            *record
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            RunningWorkState::Pending
        )
    });
    if let Some(record) = records.iter().find(|record| {
        record.key == key
            && record.cache_generation == cache_generation
            && promised_request_satisfies(&record.promise, request)
            && match &*record
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                RunningWorkState::Pending => true,
                RunningWorkState::Ready(result) => running_result_is_live(result),
                RunningWorkState::Failed => false,
            }
    }) {
        return WorkRole::Waiter(Arc::clone(record));
    }
    while records.len() >= MAX_RUNNING_WORK_RECORDS {
        let removable = records.iter().position(|record| {
            !matches!(
                *record
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                RunningWorkState::Pending
            )
        });
        let Some(index) = removable else {
            return WorkRole::Uncoordinated;
        };
        records.remove(index);
    }
    let record = Arc::new(RunningWork {
        key: key.to_owned(),
        promise: request.clone(),
        cache_generation,
        state: Mutex::new(RunningWorkState::Pending),
        ready: Condvar::new(),
    });
    records.push_back(Arc::clone(&record));
    WorkRole::Producer(record)
}

fn running_result_is_live(result: &PreviewResult) -> bool {
    result.resource.as_ref().map_or_else(
        || result.path.is_file(),
        |resource| crate::publication::shared_resource_registry().contains(&resource.resource_id),
    )
}

fn promised_request_satisfies(producer: &CacheRequest, waiter: &CacheRequest) -> bool {
    producer.source_revision == waiter.source_revision
        && producer.policy_revision == waiter.policy_revision
        && artifact_requirement_implies(producer.artifact, waiter.artifact)
        && presentation_requirement_implies(producer.presentation, waiter.presentation)
        && match (producer.detail, waiter.detail) {
            (
                DetailRequirement::NativeDetail,
                DetailRequirement::NativeDetail | DetailRequirement::Display { .. },
            ) => true,
            (
                DetailRequirement::MinimumDimensions {
                    min_long_edge: produced_long,
                    min_short_edge: produced_short,
                },
                DetailRequirement::MinimumDimensions {
                    min_long_edge: requested_long,
                    min_short_edge: requested_short,
                },
            ) => produced_long >= requested_long && produced_short >= requested_short,
            (
                DetailRequirement::MinimumDimensions {
                    min_long_edge: produced,
                    ..
                },
                DetailRequirement::Display {
                    min_long_edge: requested,
                },
            )
            | (
                DetailRequirement::Display {
                    min_long_edge: produced,
                },
                DetailRequirement::Display {
                    min_long_edge: requested,
                },
            ) => produced >= requested,
            _ => false,
        }
}

fn artifact_requirement_implies(
    producer: ArtifactRequirement,
    waiter: ArtifactRequirement,
) -> bool {
    match (producer, waiter) {
        (
            ArtifactRequirement::BoundedThumbnail { target: produced },
            ArtifactRequirement::BoundedThumbnail { target: requested },
        ) => produced == requested,
        (_, ArtifactRequirement::AnyDisplay) => true,
        (ArtifactRequirement::Exact(produced), ArtifactRequirement::Exact(requested)) => {
            produced == requested
        }
        (
            ArtifactRequirement::ExactVariant {
                origin: produced, ..
            },
            ArtifactRequirement::Exact(requested),
        ) => produced == requested,
        (
            ArtifactRequirement::ExactVariant {
                origin: produced,
                target: produced_target,
            },
            ArtifactRequirement::ExactVariant {
                origin: requested,
                target: requested_target,
            },
        ) => produced == requested && produced_target == requested_target,
        _ => false,
    }
}

fn presentation_requirement_implies(
    producer: PresentationRequirement,
    waiter: PresentationRequirement,
) -> bool {
    producer.sharpening == waiter.sharpening
        && match (producer.orientation, waiter.orientation) {
            (_, OrientationRequirement::DisplayCorrect) => true,
            (OrientationRequirement::Exact(produced), OrientationRequirement::Exact(requested)) => {
                produced == requested
            }
            (OrientationRequirement::DisplayCorrect, OrientationRequirement::Exact(_)) => false,
        }
        && match (producer.color, waiter.color) {
            (_, crate::cache::ColorRequirement::Any) => true,
            (crate::cache::ColorRequirement::Srgb, crate::cache::ColorRequirement::Srgb) => true,
            (crate::cache::ColorRequirement::Any, crate::cache::ColorRequirement::Srgb) => false,
        }
}

pub(crate) const fn applied_srgb() -> ArtifactPresentation {
    ArtifactPresentation {
        geometry: None,
        orientation: OrientationState::Applied,
        color: CacheColorState::Srgb,
        sharpening: SharpeningState::None,
    }
}

pub(crate) const fn applied_srgb_requirement() -> PresentationRequirement {
    PresentationRequirement {
        orientation: OrientationRequirement::Exact(OrientationState::Applied),
        color: crate::cache::ColorRequirement::Srgb,
        sharpening: SharpeningState::None,
    }
}

const fn domain_satisfaction(satisfaction: Satisfaction) -> oxy_domain::MediaSatisfaction {
    match satisfaction {
        Satisfaction::Satisfied => oxy_domain::MediaSatisfaction::Satisfied,
        Satisfaction::Interim => oxy_domain::MediaSatisfaction::Interim,
    }
}

fn preview_result(
    artifact: crate::cache::MediaArtifact,
    satisfaction: Satisfaction,
    level: RenderLevel,
) -> PreviewResult {
    let kind = artifact.facts.preview_kind();
    let crate::cache::ArtifactLocation::Managed(path) = artifact.location else {
        unreachable!("pipeline cache lookup contains managed artifacts")
    };
    PreviewResult {
        image_facts: Some(artifact.facts.clone()),
        geometry: artifact.variant.presentation.geometry,
        path,
        width: artifact.facts.display_dimensions.0.width,
        height: artifact.facts.display_dimensions.0.height,
        kind,
        render_level: level,
        resource: None,
        satisfaction: Some(domain_satisfaction(satisfaction)),
        persistence: Some(oxy_domain::MediaPersistence::Persisted),
        diagnostics: Some(oxy_domain::PreviewDiagnostics {
            backend: Some("cached artifact".into()),
            ..oxy_domain::PreviewDiagnostics::default()
        }),
    }
}

pub(crate) fn duration_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat};
    use std::{
        io::Cursor,
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    fn jpeg() -> Arc<[u8]> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(16, 8)
            .write_to(&mut bytes, ImageFormat::Jpeg)
            .unwrap();
        Arc::from(bytes.into_inner())
    }

    fn wait_for_thread<T>(handle: &thread::JoinHandle<T>) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !handle.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            handle.is_finished(),
            "worker thread did not finish before timeout"
        );
    }

    fn join_with_timeout<T>(handle: thread::JoinHandle<T>) -> T {
        wait_for_thread(&handle);
        handle.join().unwrap()
    }

    fn wait_until_started(started: &Arc<(Mutex<bool>, Condvar)>) {
        let (started_lock, notify) = &**started;
        let did_start = started_lock.lock().unwrap();
        let (did_start, timeout) = notify
            .wait_timeout_while(did_start, Duration::from_secs(2), |started| !*started)
            .unwrap();
        assert!(
            *did_start,
            "producer did not start before timeout: {timeout:?}"
        );
    }

    struct OpenGateOnDrop(Arc<(Mutex<bool>, Condvar)>);

    impl Drop for OpenGateOnDrop {
        fn drop(&mut self) {
            let (open, notify) = &*self.0;
            *open
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            notify.notify_all();
        }
    }

    struct DelayedStagedCache {
        inner: DiskMediaCache,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl MediaCache for DelayedStagedCache {
        fn generation(&self) -> Result<u64, MediaError> {
            self.inner.generation()
        }

        fn lookup_or_generation(
            &self,
            request: &CacheRequest,
        ) -> Result<crate::cache::CacheLookup, MediaError> {
            self.inner.lookup_or_generation(request)
        }

        fn planned_location(
            &self,
            artifact: &PendingArtifact,
        ) -> Result<Option<std::path::PathBuf>, MediaError> {
            self.inner.planned_location(artifact)
        }

        fn publish(
            &self,
            artifact: PendingArtifact,
        ) -> Result<crate::cache::CachePublication, MediaError> {
            self.inner.publish(artifact)
        }

        fn clear(&self) -> Result<(), MediaError> {
            self.inner.clear()
        }

        fn publish_staged(
            &self,
            mut artifact: crate::cache::PendingStagedArtifact,
        ) -> Result<crate::cache::CachePublication, MediaError> {
            let (open, notify) = &*self.gate;
            let mut open = open.lock().unwrap();
            while !*open {
                open = notify.wait(open).unwrap();
            }
            let staging = self.inner.root().join(".backend-tmp");
            std::fs::create_dir_all(&staging)?;
            let imported = tempfile::Builder::new()
                .suffix(".jpg")
                .tempfile_in(staging)?
                .into_temp_path();
            std::fs::copy(&artifact.staged_path, &imported)?;
            artifact.staged_path = imported
                .keep()
                .map_err(|error| MediaError::Io(error.error))?;
            self.inner.publish_staged(artifact)
        }
    }

    #[test]
    fn dimension_promises_do_not_claim_native_detail_or_an_unselected_variant() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.jpg");
        std::fs::write(&source, jpeg()).unwrap();
        let artifacts = ArtifactCache::new(&source, directory.path()).unwrap();
        let producer = artifacts.request(
            DetailRequirement::MinimumDimensions {
                min_long_edge: 900,
                min_short_edge: 600,
            },
            ArtifactRequirement::ExactVariant {
                origin: ImageOrigin::EmbeddedPreview,
                target: "selected",
            },
            applied_srgb_requirement(),
            false,
        );
        let mut waiter = producer.clone();
        assert!(promised_request_satisfies(&producer, &waiter));
        waiter.detail = DetailRequirement::NativeDetail;
        assert!(!promised_request_satisfies(&producer, &waiter));
        assert!(!promised_request_satisfies(&waiter, &producer));
        waiter.detail = DetailRequirement::Display { min_long_edge: 900 };
        waiter.artifact = ArtifactRequirement::Exact(ImageOrigin::EmbeddedPreview);
        assert!(promised_request_satisfies(&producer, &waiter));
        waiter.detail = DetailRequirement::Display { min_long_edge: 901 };
        assert!(!promised_request_satisfies(&producer, &waiter));
        waiter = producer.clone();
        waiter.detail = DetailRequirement::MinimumDimensions {
            min_long_edge: 900,
            min_short_edge: 601,
        };
        assert!(!promised_request_satisfies(&producer, &waiter));
        waiter = producer.clone();
        waiter.artifact = ArtifactRequirement::ExactVariant {
            origin: ImageOrigin::EmbeddedPreview,
            target: "different-selection",
        };
        assert!(!promised_request_satisfies(&producer, &waiter));
        waiter.artifact = ArtifactRequirement::Exact(ImageOrigin::EmbeddedPreview);
        assert!(!promised_request_satisfies(&waiter, &producer));
    }

    #[test]
    fn raw_preview_waits_on_actual_running_full_request_and_reuses_its_result() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.jpg");
        DynamicImage::new_rgb8(16, 8).save(&source).unwrap();
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let produced = Arc::new(AtomicUsize::new(0));
        let run = |wait: bool, detail: DetailRequirement, level: RenderLevel| {
            let source = source.clone();
            let cache_dir = directory.path().to_owned();
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            let produced = Arc::clone(&produced);
            thread::spawn(move || {
                let artifacts = ArtifactCache::new(&source, &cache_dir).unwrap();
                let request = if level == RenderLevel::Full {
                    crate::pipeline::raw::planner::full_developed_request(&artifacts)
                } else {
                    let DetailRequirement::Display { min_long_edge } = detail else {
                        panic!("RAW preview test requires display detail")
                    };
                    crate::pipeline::raw::planner::preview_request(&artifacts, min_long_edge, false)
                };
                artifacts
                    .coordinate_work(
                        &request,
                        level,
                        "test-development",
                        || false,
                        |generation| {
                            produced.fetch_add(1, Ordering::AcqRel);
                            if wait {
                                let (started_lock, notify) = &*started;
                                *started_lock.lock().unwrap() = true;
                                notify.notify_all();
                                let (release_lock, notify) = &*release;
                                let mut released = release_lock.lock().unwrap();
                                while !*released {
                                    released = notify.wait(released).unwrap();
                                }
                            }
                            artifacts.publish(
                                jpeg(),
                                crate::media_source::test_facts(
                                    ImageOrigin::RawSensor,
                                    PixelDimensions {
                                        width: 16,
                                        height: 8,
                                    },
                                    matches!(detail, DetailRequirement::NativeDetail),
                                ),
                                applied_srgb(),
                                "test-development".into(),
                                level,
                                generation,
                                &request,
                            )
                        },
                    )
                    .unwrap()
            })
        };
        let first = run(true, DetailRequirement::NativeDetail, RenderLevel::Full);
        wait_until_started(&started);
        let second = run(
            false,
            DetailRequirement::Display { min_long_edge: 8 },
            RenderLevel::Preview,
        );
        thread::sleep(Duration::from_millis(25));
        let (release_lock, notify) = &*release;
        *release_lock.lock().unwrap() = true;
        notify.notify_all();
        let full = join_with_timeout(first);
        let preview = join_with_timeout(second);
        assert_eq!(full.render_level, RenderLevel::Full);
        assert_eq!(preview.render_level, RenderLevel::Preview);
        assert_eq!(produced.load(Ordering::Acquire), 1);
    }

    #[test]
    fn heif_preview_waits_on_actual_running_full_request_and_reuses_its_result() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.heif");
        std::fs::write(&source, jpeg()).unwrap();
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let produced = Arc::new(AtomicUsize::new(0));
        let run = |wait: bool, detail: DetailRequirement, level: RenderLevel| {
            let source = source.clone();
            let cache_dir = directory.path().to_owned();
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            let produced = Arc::clone(&produced);
            thread::spawn(move || {
                let artifacts = ArtifactCache::new(&source, &cache_dir).unwrap();
                let request = if level == RenderLevel::Full {
                    crate::pipeline::heif::planner::full_decoded_request(&artifacts)
                } else {
                    crate::pipeline::heif::planner::preview_request(&artifacts, detail, false)
                };
                artifacts
                    .coordinate_work(
                        &request,
                        level,
                        "test-heif-source-decode",
                        || false,
                        |generation| {
                            produced.fetch_add(1, Ordering::AcqRel);
                            if wait {
                                let (started_lock, notify) = &*started;
                                *started_lock.lock().unwrap() = true;
                                notify.notify_all();
                                let (release_lock, notify) = &*release;
                                let mut released = release_lock.lock().unwrap();
                                while !*released {
                                    released = notify.wait(released).unwrap();
                                }
                            }
                            artifacts.publish(
                                jpeg(),
                                crate::media_source::test_facts(
                                    ImageOrigin::PrimaryImage,
                                    PixelDimensions {
                                        width: 16,
                                        height: 8,
                                    },
                                    matches!(detail, DetailRequirement::NativeDetail),
                                ),
                                ArtifactPresentation {
                                    geometry: None,
                                    orientation: OrientationState::Applied,
                                    color: CacheColorState::EmbeddedOrUnknown,
                                    sharpening: SharpeningState::None,
                                },
                                "test-heif-source-decode".into(),
                                level,
                                generation,
                                &request,
                            )
                        },
                    )
                    .unwrap()
            })
        };
        let full = run(true, DetailRequirement::NativeDetail, RenderLevel::Full);
        wait_until_started(&started);
        let preview = run(
            false,
            DetailRequirement::Display { min_long_edge: 8 },
            RenderLevel::Preview,
        );
        thread::sleep(Duration::from_millis(25));
        let (release_lock, notify) = &*release;
        *release_lock.lock().unwrap() = true;
        notify.notify_all();
        assert_eq!(join_with_timeout(full).render_level, RenderLevel::Full);
        assert_eq!(
            join_with_timeout(preview).render_level,
            RenderLevel::Preview
        );
        assert_eq!(produced.load(Ordering::Acquire), 1);
    }

    #[test]
    fn panicking_producer_marks_running_work_failed_and_wakes_waiter() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("panic-source.heif");
        std::fs::write(&source, jpeg()).unwrap();
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let producer = {
            let source = source.clone();
            let cache_dir = directory.path().to_owned();
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            thread::spawn(move || {
                let artifacts = ArtifactCache::new(&source, &cache_dir).unwrap();
                let request = crate::pipeline::heif::planner::full_decoded_request(&artifacts);
                let _ = artifacts.coordinate_work(
                    &request,
                    RenderLevel::Full,
                    "panic-heif-source-decode",
                    || false,
                    |_| {
                        let (started_lock, notify) = &*started;
                        *started_lock.lock().unwrap() = true;
                        notify.notify_all();
                        let (release_lock, notify) = &*release;
                        let released = release_lock.lock().unwrap();
                        let _released = notify
                            .wait_timeout_while(released, Duration::from_secs(2), |open| !*open)
                            .unwrap();
                        panic!("intentional producer panic")
                    },
                );
            })
        };
        wait_until_started(&started);
        let waiter = {
            let cache_dir = directory.path().to_owned();
            thread::spawn(move || {
                let artifacts = ArtifactCache::new(&source, &cache_dir).unwrap();
                let request = crate::pipeline::heif::planner::preview_request(
                    &artifacts,
                    DetailRequirement::Display { min_long_edge: 8 },
                    false,
                );
                artifacts.coordinate_work(
                    &request,
                    RenderLevel::Preview,
                    "panic-heif-source-decode",
                    || false,
                    |generation| {
                        artifacts.publish(
                            jpeg(),
                            crate::media_source::test_facts(
                                ImageOrigin::PrimaryImage,
                                PixelDimensions {
                                    width: 16,
                                    height: 8,
                                },
                                true,
                            ),
                            ArtifactPresentation {
                                geometry: None,
                                orientation: OrientationState::Applied,
                                color: CacheColorState::EmbeddedOrUnknown,
                                sharpening: SharpeningState::None,
                            },
                            "panic-heif-source-decode".into(),
                            RenderLevel::Preview,
                            generation,
                            &request,
                        )
                    },
                )
            })
        };
        thread::sleep(Duration::from_millis(25));
        let (release_lock, notify) = &*release;
        *release_lock.lock().unwrap() = true;
        notify.notify_all();

        wait_for_thread(&producer);
        assert!(producer.join().is_err());
        assert_eq!(
            join_with_timeout(waiter).unwrap().render_level,
            RenderLevel::Preview
        );
    }

    #[test]
    fn queue_backpressure_does_not_break_running_result_handoff() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source-backpressure.jpg");
        DynamicImage::new_rgb8(16, 8).save(&source).unwrap();
        let publisher = Arc::new(crate::publication::ArtifactPublisher::new(
            crate::publication::ResourceRegistry::new(
                crate::publication::ResourceRegistryLimits::new(4, 1024 * 1024),
            ),
            Arc::new(DiskMediaCache::new(directory.path(), 8).unwrap()),
            1,
            0,
            1,
        ));
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let produced = Arc::new(AtomicUsize::new(0));
        let run = |wait: bool, detail: DetailRequirement, level: RenderLevel| {
            let source = source.clone();
            let cache_dir = directory.path().to_owned();
            let publisher = Arc::clone(&publisher);
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            let produced = Arc::clone(&produced);
            thread::spawn(move || {
                let artifacts = ArtifactCache::new(&source, &cache_dir).unwrap();
                let request = artifacts.request(
                    detail,
                    ArtifactRequirement::Exact(ImageOrigin::RawSensor),
                    applied_srgb_requirement(),
                    false,
                );
                artifacts
                    .coordinate_work(
                        &request,
                        level,
                        "backpressure-development",
                        || false,
                        |_generation| {
                            produced.fetch_add(1, Ordering::AcqRel);
                            if wait {
                                let (started_lock, notify) = &*started;
                                *started_lock.lock().unwrap() = true;
                                notify.notify_all();
                                let (release_lock, notify) = &*release;
                                let mut released = release_lock.lock().unwrap();
                                while !*released {
                                    released = notify.wait(released).unwrap();
                                }
                            }
                            let published = publisher.publish(
                                crate::publication::ProducedArtifact {
                                    source_revision: request.source_revision.clone(),
                                    variant: VariantIdentity {
                                        presentation: applied_srgb(),
                                        policy_revision: MEDIA_CACHE_POLICY_REVISION,
                                        target: "backpressure-development".into(),
                                    },
                                    facts: crate::media_source::test_facts(
                                        ImageOrigin::RawSensor,
                                        PixelDimensions {
                                            width: 16,
                                            height: 8,
                                        },
                                        true,
                                    ),
                                    cache_generation: 0,
                                    payload: crate::publication::ProducedPayload::Encoded {
                                        bytes: jpeg(),
                                        media_type: "image/jpeg".into(),
                                        extension: "jpg".into(),
                                    },
                                },
                                true,
                            )?;
                            assert_eq!(
                                published.persistence,
                                crate::publication::PersistenceStatus::SkippedBackpressure
                            );
                            Ok(PreviewResult {
                                image_facts: None,
                                geometry: None,
                                path: std::path::PathBuf::new(),
                                width: 16,
                                height: 8,
                                kind: PreviewKind::Developed,
                                render_level: level,
                                resource: Some(oxy_domain::MediaResourceDescriptor {
                                    resource_id: published.resource.descriptor.resource_id.clone(),
                                    url: published.resource.descriptor.url.clone(),
                                    media_type: published.resource.descriptor.media_type,
                                }),
                                satisfaction: Some(oxy_domain::MediaSatisfaction::Satisfied),
                                persistence: Some(oxy_domain::MediaPersistence::Skipped),
                                diagnostics: None,
                            })
                        },
                    )
                    .unwrap()
            })
        };
        let full = run(true, DetailRequirement::NativeDetail, RenderLevel::Full);
        wait_until_started(&started);
        let preview = run(
            false,
            DetailRequirement::Display { min_long_edge: 8 },
            RenderLevel::Preview,
        );
        thread::sleep(Duration::from_millis(25));
        let (release_lock, notify) = &*release;
        *release_lock.lock().unwrap() = true;
        notify.notify_all();
        assert_eq!(
            join_with_timeout(full).persistence,
            Some(oxy_domain::MediaPersistence::Skipped)
        );
        assert_eq!(
            join_with_timeout(preview).render_level,
            RenderLevel::Preview
        );
        assert_eq!(produced.load(Ordering::Acquire), 1);
    }

    #[test]
    fn staged_running_handoff_subscribes_every_app_waiter_to_persistence() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source-staged.heif");
        std::fs::write(&source, jpeg()).unwrap();
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let persistence_gate = Arc::new((Mutex::new(false), Condvar::new()));
        let delayed_cache = Arc::new(DelayedStagedCache {
            inner: DiskMediaCache::new(directory.path(), 8).unwrap(),
            gate: Arc::clone(&persistence_gate),
        });
        let publisher = Arc::new(crate::publication::ArtifactPublisher::new(
            crate::publication::shared_resource_registry(),
            delayed_cache,
            2,
            1024 * 1024,
            1,
        ));
        let _open_gate_on_drop = OpenGateOnDrop(Arc::clone(&persistence_gate));
        let run = |producer: bool, level: RenderLevel| {
            let source = source.clone();
            let cache_dir = directory.path().to_owned();
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            let publisher = Arc::clone(&publisher);
            thread::spawn(move || {
                with_app_publication(|| {
                    let mut artifacts = ArtifactCache::new(&source, &cache_dir).unwrap();
                    artifacts.publisher = Some(publisher);
                    let detail = if level == RenderLevel::Full {
                        DetailRequirement::NativeDetail
                    } else {
                        DetailRequirement::Display { min_long_edge: 8 }
                    };
                    let request = if level == RenderLevel::Full {
                        crate::pipeline::heif::planner::full_decoded_request(&artifacts)
                    } else {
                        crate::pipeline::heif::planner::preview_request(&artifacts, detail, false)
                    };
                    artifacts.coordinate_work(
                        &request,
                        level,
                        "staged-heif-source-decode",
                        || false,
                        |generation| {
                            assert!(producer, "waiter unexpectedly decoded the source");
                            let (started_lock, notify) = &*started;
                            *started_lock.lock().unwrap() = true;
                            notify.notify_all();
                            let (release_lock, notify) = &*release;
                            let mut released = release_lock.lock().unwrap();
                            while !*released {
                                released = notify.wait(released).unwrap();
                            }
                            drop(released);

                            let output = artifacts.temporary_output(".jpg").unwrap();
                            DynamicImage::new_rgb8(16, 8).save(&*output).unwrap();
                            artifacts.publish_staged(
                                output,
                                crate::media_source::test_facts(
                                    ImageOrigin::PrimaryImage,
                                    PixelDimensions {
                                        width: 16,
                                        height: 8,
                                    },
                                    true,
                                ),
                                ArtifactPresentation {
                                    geometry: None,
                                    orientation: OrientationState::Applied,
                                    color: CacheColorState::EmbeddedOrUnknown,
                                    sharpening: SharpeningState::None,
                                },
                                "staged-heif-source-decode".into(),
                                level,
                                generation,
                                &request,
                            )
                        },
                    )
                })
            })
        };

        let producer = run(true, RenderLevel::Full);
        wait_until_started(&started);
        let waiter = run(false, RenderLevel::Preview);
        thread::sleep(Duration::from_millis(25));
        let (release_lock, notify) = &*release;
        *release_lock.lock().unwrap() = true;
        notify.notify_all();

        let (producer_result, producer_completions) = join_with_timeout(producer);
        let (waiter_result, waiter_completions) = join_with_timeout(waiter);
        assert_eq!(
            producer_result.unwrap().persistence,
            Some(oxy_domain::MediaPersistence::Pending)
        );
        assert_eq!(
            waiter_result.unwrap().persistence,
            Some(oxy_domain::MediaPersistence::Pending)
        );
        assert_eq!(producer_completions.len(), 1);
        assert_eq!(waiter_completions.len(), 1);

        let (open, notify) = &*persistence_gate;
        *open.lock().unwrap() = true;
        notify.notify_all();
        for completion in producer_completions.into_iter().chain(waiter_completions) {
            let completion = completion.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(
                matches!(
                    completion,
                    crate::publication::PersistenceCompletion::Persisted { .. }
                ),
                "persistence unexpectedly failed: {completion:?}"
            );
        }
    }

    #[test]
    fn live_backend_output_survives_prune_and_clear_until_released() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.jpg");
        DynamicImage::new_rgb8(16, 8).save(&source).unwrap();
        let artifacts = ArtifactCache::new(&source, directory.path()).unwrap();
        let output = artifacts.temporary_output(".jpg").unwrap();
        DynamicImage::new_rgb8(16, 8).save(&*output).unwrap();

        let maintenance = DiskMediaCache::new(directory.path(), 8).unwrap();
        maintenance.prune(0).unwrap();
        assert!(output.is_file());
        maintenance.clear().unwrap();
        assert!(output.is_file());

        let path = output.to_path_buf();
        drop(output);
        assert!(!path.exists());
    }
}
