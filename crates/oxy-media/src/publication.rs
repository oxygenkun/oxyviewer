use crate::{
    MediaError,
    cache::{
        ArtifactLease, ArtifactLocation, CacheRequest, ImageOrigin, MediaArtifact, MediaCache,
        PendingArtifact, PixelDimensions, Satisfaction, SourceRevision, VariantIdentity,
        candidate_rank, satisfies,
    },
};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

mod limits;
#[cfg(test)]
mod limits_tests;
pub use limits::{ResourceRegistryLimits, resource_memory_budget};

static RESOURCE_PROCESS_NAMESPACE: OnceLock<String> = OnceLock::new();
static RESOURCE_ID: AtomicU64 = AtomicU64::new(0);

fn resource_process_namespace() -> &'static str {
    RESOURCE_PROCESS_NAMESPACE.get_or_init(|| {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        format!("{:x}-{started:x}", std::process::id())
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceDescriptor {
    pub resource_id: String,
    pub url: String,
    pub dimensions: PixelDimensions,
    pub origin: ImageOrigin,
    pub media_type: String,
}

pub struct ResourceHandle {
    pub descriptor: ResourceDescriptor,
    _lease: ResourceLease,
}

/// RAII ownership for a native artifact staged outside the managed cache.
/// Every failure path deletes it; registry and persistence worker share this
/// owner until the resource has atomically transitioned to its managed file.
#[derive(Debug)]
pub struct OwnedStagedFile {
    path: PathBuf,
}

impl OwnedStagedFile {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for OwnedStagedFile {
    fn drop(&mut self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "failed to clean staged media artifact {}: {error}",
                self.path.display()
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ResourcePayload {
    Encoded(Arc<[u8]>),
    File(PathBuf),
}

pub struct ResourceReadLease {
    pub media_type: String,
    pub dimensions: PixelDimensions,
    pub origin: ImageOrigin,
    pub payload: ResourcePayload,
    file_revision: Option<SourceRevision>,
    _lease: ResourceLease,
}

pub struct ResourceLease {
    registry: Arc<RegistryInner>,
    token: u64,
}

impl Drop for ResourceLease {
    fn drop(&mut self) {
        self.registry.release(self.token);
    }
}

#[derive(Clone)]
pub struct ResourceRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    state: Mutex<RegistryState>,
    limits: ResourceRegistryLimits,
    materialized_responses: AtomicUsize,
    materialized_bytes: AtomicUsize,
}

#[derive(Default)]
struct RegistryState {
    entries: HashMap<String, RegistryEntry>,
    lru: VecDeque<String>,
    leases: HashMap<u64, String>,
    next_lease: u64,
    memory_bytes: usize,
    peak_memory_bytes: usize,
    peak_entries: usize,
}

pub use oxy_domain::ResourceRegistryStats;

fn registry_stats(state: &RegistryState) -> ResourceRegistryStats {
    let now = Instant::now();
    let mut stats = ResourceRegistryStats {
        entries: state.entries.len(),
        encoded_bytes: state.memory_bytes,
        peak_encoded_bytes: state.peak_memory_bytes,
        peak_entries: state.peak_entries,
        ..ResourceRegistryStats::default()
    };
    for (id, entry) in &state.entries {
        if entry.ui_lease_until > now {
            stats.ui_leased += 1;
        } else {
            stats.released_or_expired += 1;
        }
        if state.leases.values().any(|leased| leased == id) {
            stats.read_leased += 1;
        }
        if entry.staged_owner.is_some() {
            stats.staged_files += 1;
            stats.staged_bytes = stats.staged_bytes.saturating_add(
                entry
                    .file_revision
                    .as_ref()
                    .map_or(0, |revision| revision.size_bytes),
            );
        }
    }
    stats
}

fn budget_error(
    budget: &'static str,
    current: usize,
    limit: usize,
    requested: usize,
) -> MediaError {
    MediaError::ResourceBudgetExhausted {
        budget,
        current,
        limit,
        requested,
    }
}

fn registry_budget_error(
    state: &RegistryState,
    registry: &RegistryInner,
    bytes: usize,
    entries: usize,
) -> MediaError {
    let error = if state
        .entries
        .len()
        .checked_add(entries)
        .is_none_or(|count| count > registry.limits.max_entries)
    {
        budget_error(
            "entry",
            state.entries.len(),
            registry.limits.max_entries,
            entries,
        )
    } else {
        budget_error(
            "encoded memory",
            state.memory_bytes,
            registry.limits.max_encoded_bytes,
            bytes,
        )
    };
    eprintln!(
        "artifact publication rejected: {error}; registry={:?}",
        registry_stats(state)
    );
    error
}

struct RegistryEntry {
    dimensions: PixelDimensions,
    origin: ImageOrigin,
    media_type: String,
    payload: ResourcePayload,
    memory_bytes: usize,
    ui_lease_until: Instant,
    file_revision: Option<SourceRevision>,
    artifact_lease: Option<ArtifactLease>,
    staged_owner: Option<Arc<OwnedStagedFile>>,
}

static SHARED_REGISTRY: OnceLock<ResourceRegistry> = OnceLock::new();
static SHARED_PUBLISHERS: OnceLock<Mutex<HashMap<PathBuf, Arc<ArtifactPublisher>>>> =
    OnceLock::new();

pub fn shared_resource_registry() -> ResourceRegistry {
    SHARED_REGISTRY
        .get_or_init(|| ResourceRegistry::new(ResourceRegistryLimits::production()))
        .clone()
}

pub(crate) fn shared_publisher(cache_dir: &Path) -> Result<Arc<ArtifactPublisher>, MediaError> {
    let canonical_parent = cache_dir.canonicalize().or_else(|_| {
        std::fs::create_dir_all(cache_dir)?;
        cache_dir.canonicalize()
    })?;
    let publishers = SHARED_PUBLISHERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut publishers = publishers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(publisher) = publishers.get(&canonical_parent) {
        return Ok(Arc::clone(publisher));
    }
    let cache: Arc<dyn MediaCache> =
        Arc::new(crate::cache::DiskMediaCache::new(&canonical_parent, 256)?);
    let publisher = Arc::new(ArtifactPublisher::new_with_staging_parent(
        shared_resource_registry(),
        cache,
        8,
        256 * 1024 * 1024,
        1,
        canonical_parent.clone(),
    ));
    publishers.insert(canonical_parent, Arc::clone(&publisher));
    Ok(publisher)
}

impl ResourceRegistry {
    pub fn new(limits: ResourceRegistryLimits) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                state: Mutex::new(RegistryState::default()),
                limits,
                materialized_responses: AtomicUsize::new(0),
                materialized_bytes: AtomicUsize::new(0),
            }),
        }
    }

    pub fn register_encoded(
        &self,
        bytes: Arc<[u8]>,
        media_type: impl Into<String>,
        dimensions: PixelDimensions,
        origin: ImageOrigin,
    ) -> Result<ResourceHandle, MediaError> {
        self.register(
            ResourcePayload::Encoded(bytes),
            media_type.into(),
            dimensions,
            origin,
            None,
            None,
            None,
        )
    }

    pub fn register_file(
        &self,
        path: &Path,
        media_type: impl Into<String>,
        dimensions: PixelDimensions,
        origin: ImageOrigin,
    ) -> Result<ResourceHandle, MediaError> {
        self.register_file_with_lease(path, media_type, dimensions, origin, None)
    }

    pub fn register_file_with_lease(
        &self,
        path: &Path,
        media_type: impl Into<String>,
        dimensions: PixelDimensions,
        origin: ImageOrigin,
        artifact_lease: Option<ArtifactLease>,
    ) -> Result<ResourceHandle, MediaError> {
        let revision = oxy_fs::observe_source_revision(path)?;
        self.register(
            ResourcePayload::File(revision.canonical_path.clone()),
            media_type.into(),
            dimensions,
            origin,
            Some(revision),
            artifact_lease,
            None,
        )
    }

    pub fn register_owned_staged_file(
        &self,
        owner: Arc<OwnedStagedFile>,
        media_type: impl Into<String>,
        dimensions: PixelDimensions,
        origin: ImageOrigin,
    ) -> Result<ResourceHandle, MediaError> {
        let revision = oxy_fs::observe_source_revision(owner.path())?;
        self.register(
            ResourcePayload::File(revision.canonical_path.clone()),
            media_type.into(),
            dimensions,
            origin,
            Some(revision),
            None,
            Some(owner),
        )
    }

    pub fn contains(&self, resource_id: &str) -> bool {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .contains_key(resource_id)
    }

    pub fn resolve(&self, resource_id: &str) -> Option<ResourceReadLease> {
        let (media_type, dimensions, origin, payload, file_revision, lease) =
            self.inner.resolve(resource_id)?;
        Some(ResourceReadLease {
            media_type,
            dimensions,
            origin,
            payload,
            file_revision,
            _lease: lease,
        })
    }

    /// Materializes a protocol response under explicit concurrency and byte
    /// reservations. Tauri currently requires a `Vec<u8>` protocol body, so
    /// file responses cannot be streamed on every platform.
    pub fn materialize(&self, resource: &ResourceReadLease) -> Result<Vec<u8>, MediaError> {
        let byte_size = match &resource.payload {
            ResourcePayload::Encoded(bytes) => bytes.len(),
            ResourcePayload::File(path) => {
                usize::try_from(fs::metadata(path)?.len()).map_err(|_| {
                    budget_error(
                        "materialized bytes",
                        self.inner.materialized_bytes.load(Ordering::Acquire),
                        self.inner.limits.max_materialized_bytes,
                        usize::MAX,
                    )
                })?
            }
        };
        let _reservation = MaterializedReservation::acquire(&self.inner, byte_size)?;
        let bytes = match &resource.payload {
            ResourcePayload::Encoded(bytes) => bytes.to_vec(),
            ResourcePayload::File(path) => {
                // Never grow a response beyond its reservation if an original
                // file changes between metadata observation and reading.
                let mut file = fs::File::open(path)?;
                let mut bytes = vec![0; byte_size];
                file.read_exact(&mut bytes)?;
                if file.read(&mut [0])? != 0 {
                    return Err(MediaError::StaleSourceRevision);
                }
                bytes
            }
        };
        if let Some(expected) = &resource.file_revision {
            let actual = oxy_fs::observe_source_revision(&expected.canonical_path)?;
            if actual != *expected {
                return Err(MediaError::StaleSourceRevision);
            }
        }
        Ok(bytes)
    }

    pub fn transition_to_managed_file(
        &self,
        resource_id: &str,
        path: &Path,
        artifact_lease: &mut Option<ArtifactLease>,
    ) -> Result<bool, MediaError> {
        let revision = oxy_fs::observe_source_revision(path)?;
        let old_staged_owner = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state
                .leases
                .values()
                .any(|leased_resource| leased_resource == resource_id)
            {
                return Ok(false);
            }
            let Some(entry) = state.entries.get_mut(resource_id) else {
                return Err(MediaError::CacheArtifact(
                    "resource expired before managed publication transition".into(),
                ));
            };
            let old_staged_owner = entry.staged_owner.take();
            let released_memory = entry.memory_bytes;
            entry.payload = ResourcePayload::File(revision.canonical_path.clone());
            entry.file_revision = Some(revision);
            entry.artifact_lease = artifact_lease.take();
            entry.memory_bytes = 0;
            state.memory_bytes = state.memory_bytes.saturating_sub(released_memory);
            old_staged_owner
        };
        // The registry transition is already complete. Cleanup belongs to the
        // RAII owner and cannot turn a valid managed publication into Failed.
        drop(old_staged_owner);
        Ok(true)
    }

    /// Republishing a live descriptor covers its new IPC handoff, without
    /// claiming a long UI lease or shortening an existing active lease.
    pub fn refresh_publication_grace(&self, resource_id: &str) -> bool {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = state.entries.get_mut(resource_id) else {
            return false;
        };
        entry.ui_lease_until = entry
            .ui_lease_until
            .max(Instant::now() + self.inner.limits.publish_grace);
        true
    }

    pub fn renew(&self, resource_id: &str) -> bool {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = state.entries.get_mut(resource_id) else {
            return false;
        };
        entry.ui_lease_until = Instant::now() + self.inner.limits.ui_lease;
        if let Some(lease) = &entry.artifact_lease
            && lease.renew().is_err()
        {
            remove_entry(&mut state, resource_id);
            return false;
        }
        state.lru.retain(|candidate| candidate != resource_id);
        state.lru.push_back(resource_id.to_owned());
        remove_expired(&mut state);
        true
    }

    /// Releases the UI lease and makes this immutable resource the first
    /// eviction candidate. An active protocol read remains protected.
    pub fn release(&self, resource_id: &str) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = state.entries.get_mut(resource_id) {
            entry.ui_lease_until = Instant::now();
            state.lru.retain(|candidate| candidate != resource_id);
            state.lru.push_front(resource_id.to_owned());
        }
    }

    pub fn limits(&self) -> ResourceRegistryLimits {
        self.inner.limits
    }

    pub fn stats(&self) -> ResourceRegistryStats {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ResourceRegistryStats {
            max_entries: self.inner.limits.max_entries,
            max_encoded_bytes: self.inner.limits.max_encoded_bytes,
            max_materialized_responses: self.inner.limits.max_materialized_responses,
            max_materialized_bytes: self.inner.limits.max_materialized_bytes,
            materialized_responses: self.inner.materialized_responses.load(Ordering::Acquire),
            materialized_bytes: self.inner.materialized_bytes.load(Ordering::Acquire),
            ..registry_stats(&state)
        }
    }

    pub fn len(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[allow(clippy::too_many_arguments)]
    fn register(
        &self,
        payload: ResourcePayload,
        media_type: String,
        dimensions: PixelDimensions,
        origin: ImageOrigin,
        file_revision: Option<SourceRevision>,
        artifact_lease: Option<ArtifactLease>,
        staged_owner: Option<Arc<OwnedStagedFile>>,
    ) -> Result<ResourceHandle, MediaError> {
        let memory_bytes = match &payload {
            ResourcePayload::Encoded(bytes) => bytes.len(),
            ResourcePayload::File(_) => 0,
        };
        let id = format!(
            "resource-{}-{}",
            resource_process_namespace(),
            RESOURCE_ID.fetch_add(1, Ordering::Relaxed) + 1
        );
        let entry = RegistryEntry {
            dimensions,
            origin,
            media_type: media_type.clone(),
            payload,
            memory_bytes,
            ui_lease_until: Instant::now() + self.inner.limits.publish_grace,
            file_revision,
            artifact_lease,
            staged_owner,
        };
        let lease = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            evict_for(&mut state, &self.inner, memory_bytes, 1)?;
            state.memory_bytes = state.memory_bytes.saturating_add(memory_bytes);
            state.entries.insert(id.clone(), entry);
            state.peak_memory_bytes = state.peak_memory_bytes.max(state.memory_bytes);
            state.peak_entries = state.peak_entries.max(state.entries.len());
            state.lru.push_back(id.clone());
            self.inner.acquire_locked(&mut state, &id)
        };
        Ok(ResourceHandle {
            descriptor: ResourceDescriptor {
                resource_id: id.clone(),
                url: format!("oxy-media://localhost/resource/{id}"),
                dimensions,
                origin,
                media_type,
            },
            _lease: lease,
        })
    }
}

struct MaterializedReservation {
    registry: Arc<RegistryInner>,
    byte_size: usize,
}

impl MaterializedReservation {
    fn acquire(registry: &Arc<RegistryInner>, byte_size: usize) -> Result<Self, MediaError> {
        if byte_size > registry.limits.max_materialized_bytes {
            return Err(budget_error(
                "materialized bytes",
                registry.materialized_bytes.load(Ordering::Acquire),
                registry.limits.max_materialized_bytes,
                byte_size,
            ));
        }
        let prior_count = registry
            .materialized_responses
            .fetch_add(1, Ordering::AcqRel);
        if prior_count >= registry.limits.max_materialized_responses {
            registry
                .materialized_responses
                .fetch_sub(1, Ordering::AcqRel);
            return Err(budget_error(
                "materialized count",
                prior_count,
                registry.limits.max_materialized_responses,
                1,
            ));
        }
        if !reserve_pending_bytes(
            &registry.materialized_bytes,
            registry.limits.max_materialized_bytes,
            byte_size,
        ) {
            registry
                .materialized_responses
                .fetch_sub(1, Ordering::AcqRel);
            return Err(budget_error(
                "materialized bytes",
                registry.materialized_bytes.load(Ordering::Acquire),
                registry.limits.max_materialized_bytes,
                byte_size,
            ));
        }
        Ok(Self {
            registry: Arc::clone(registry),
            byte_size,
        })
    }
}

impl Drop for MaterializedReservation {
    fn drop(&mut self) {
        self.registry
            .materialized_bytes
            .fetch_sub(self.byte_size, Ordering::AcqRel);
        self.registry
            .materialized_responses
            .fetch_sub(1, Ordering::AcqRel);
    }
}

impl RegistryInner {
    fn acquire_locked(self: &Arc<Self>, state: &mut RegistryState, id: &str) -> ResourceLease {
        state.next_lease = state.next_lease.wrapping_add(1);
        let token = state.next_lease;
        state.leases.insert(token, id.to_owned());
        ResourceLease {
            registry: Arc::clone(self),
            token,
        }
    }

    fn resolve(
        self: &Arc<Self>,
        id: &str,
    ) -> Option<(
        String,
        PixelDimensions,
        ImageOrigin,
        ResourcePayload,
        Option<SourceRevision>,
        ResourceLease,
    )> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let immutable = state
            .entries
            .get(id)?
            .file_revision
            .as_ref()
            .is_none_or(|expected| {
                oxy_fs::observe_source_revision(&expected.canonical_path)
                    .is_ok_and(|actual| actual == *expected)
            });
        if !immutable {
            remove_entry(&mut state, id);
            return None;
        }
        let entry = state.entries.get(id)?;
        let resolved = (
            entry.media_type.clone(),
            entry.dimensions,
            entry.origin,
            entry.payload.clone(),
            entry.file_revision.clone(),
        );
        state.lru.retain(|candidate| candidate != id);
        state.lru.push_back(id.to_owned());
        let lease = self.acquire_locked(&mut state, id);
        Some((
            resolved.0, resolved.1, resolved.2, resolved.3, resolved.4, lease,
        ))
    }

    fn release(&self, token: u64) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .leases
            .remove(&token);
    }
}

fn remove_expired(state: &mut RegistryState) {
    // Drop obsolete file owners and encoded buffers on the next publication,
    // even when capacity remains. Active UI and protocol readers are protected.
    let now = Instant::now();
    let expired: Vec<_> = state
        .lru
        .iter()
        .filter(|id| {
            state
                .entries
                .get(*id)
                .is_some_and(|entry| entry.ui_lease_until <= now)
                && !state.leases.values().any(|leased| leased == *id)
        })
        .cloned()
        .collect();
    for id in expired {
        remove_entry(state, &id);
    }
}

fn evict_for(
    state: &mut RegistryState,
    limits: &RegistryInner,
    additional_bytes: usize,
    additional_entries: usize,
) -> Result<(), MediaError> {
    remove_expired(state);
    while state
        .entries
        .len()
        .checked_add(additional_entries)
        .is_none_or(|count| count > limits.limits.max_entries)
        || state
            .memory_bytes
            .checked_add(additional_bytes)
            .is_none_or(|bytes| bytes > limits.limits.max_encoded_bytes)
    {
        let Some(candidate) = state.lru.pop_front() else {
            return Err(registry_budget_error(
                state,
                limits,
                additional_bytes,
                additional_entries,
            ));
        };
        let has_read_lease = state.leases.values().any(|leased| leased == &candidate);
        let has_ui_lease = state
            .entries
            .get(&candidate)
            .is_some_and(|entry| entry.ui_lease_until > Instant::now());
        if has_read_lease || has_ui_lease {
            state.lru.push_back(candidate);
            if state.lru.iter().all(|id| {
                state.leases.values().any(|leased| leased == id)
                    || state
                        .entries
                        .get(id)
                        .is_some_and(|entry| entry.ui_lease_until > Instant::now())
            }) {
                return Err(registry_budget_error(
                    state,
                    limits,
                    additional_bytes,
                    additional_entries,
                ));
            }
            continue;
        }
        remove_entry(state, &candidate);
    }
    Ok(())
}

fn remove_entry(state: &mut RegistryState, id: &str) {
    if let Some(entry) = state.entries.remove(id) {
        state.memory_bytes = state.memory_bytes.saturating_sub(entry.memory_bytes);
    }
    state.lru.retain(|candidate| candidate != id);
}

#[derive(Clone)]
pub struct ProducedArtifact {
    pub source_revision: SourceRevision,
    pub variant: VariantIdentity,
    pub facts: oxy_domain::ArtifactFacts,
    pub cache_generation: u64,
    pub payload: ProducedPayload,
}

#[derive(Clone)]
pub enum ProducedPayload {
    OriginalFile {
        path: PathBuf,
        media_type: String,
    },
    Encoded {
        bytes: Arc<[u8]>,
        media_type: String,
        extension: String,
    },
    StagedFile {
        owner: Arc<OwnedStagedFile>,
        media_type: String,
        extension: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistenceStatus {
    NotRequested,
    Scheduled,
    Persisted,
    SkippedBackpressure,
}

pub struct PublishedArtifact {
    pub resource: ResourceHandle,
    pub managed_path: Option<PathBuf>,
    pub persistence: PersistenceStatus,
    pub completion: Option<Receiver<PersistenceCompletion>>,
}

#[derive(Debug)]
pub struct ActivePublicationHit {
    pub artifact: MediaArtifact,
    pub resource: ResourceDescriptor,
    pub satisfaction: Satisfaction,
    pub persistence: PersistenceStatus,
    pub completion: Option<Receiver<PersistenceCompletion>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistenceCompletion {
    Persisted {
        resource_id: String,
        path: PathBuf,
    },
    Failed {
        resource_id: String,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistenceFailure {
    pub source_revision_id: String,
    pub message: String,
}

enum PendingPersistence {
    Encoded(PendingArtifact),
    Staged {
        pending: crate::cache::PendingStagedArtifact,
        _owner: Arc<OwnedStagedFile>,
    },
}

struct PersistenceJob {
    pending: PendingPersistence,
    resource_id: String,
    memory_bytes: usize,
    completion: mpsc::Sender<PersistenceCompletion>,
}

/// Publishes a display resource first, then offers the same shared encoded
/// bytes to a bounded cache queue. A full queue skips rebuildable persistence;
/// it never discards the UI resource.
pub struct ArtifactPublisher {
    registry: ResourceRegistry,
    cache: Arc<dyn MediaCache>,
    sender: Option<SyncSender<PersistenceJob>>,
    workers: Vec<JoinHandle<()>>,
    failures: Arc<Mutex<Vec<PersistenceFailure>>>,
    active: Arc<Mutex<HashMap<String, Vec<ActivePublication>>>>,
    pending_bytes: Arc<AtomicUsize>,
    max_pending_bytes: usize,
    staging_parent: PathBuf,
    staging: Mutex<Option<tempfile::TempDir>>,
}

#[derive(Clone)]
struct ActivePublication {
    artifact: MediaArtifact,
    resource: ResourceDescriptor,
    persistence: PersistenceStatus,
    cache_generation: u64,
    subscribers: Vec<mpsc::Sender<PersistenceCompletion>>,
}

impl ArtifactPublisher {
    pub fn new(
        registry: ResourceRegistry,
        cache: Arc<dyn MediaCache>,
        queue_capacity: usize,
        max_pending_bytes: usize,
        worker_count: usize,
    ) -> Self {
        Self::new_with_staging_parent(
            registry,
            cache,
            queue_capacity,
            max_pending_bytes,
            worker_count,
            std::env::temp_dir(),
        )
    }

    fn new_with_staging_parent(
        registry: ResourceRegistry,
        cache: Arc<dyn MediaCache>,
        queue_capacity: usize,
        max_pending_bytes: usize,
        worker_count: usize,
        staging_parent: PathBuf,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<PersistenceJob>(queue_capacity.max(1));
        let receiver = Arc::new(Mutex::new(receiver));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let active = Arc::new(Mutex::new(HashMap::<String, Vec<ActivePublication>>::new()));
        let pending_bytes = Arc::new(AtomicUsize::new(0));
        let workers = (0..worker_count.max(1))
            .map(|_| {
                let receiver = Arc::clone(&receiver);
                let failures = Arc::clone(&failures);
                let pending_bytes = Arc::clone(&pending_bytes);
                let cache = Arc::clone(&cache);
                let registry = registry.clone();
                let active = Arc::clone(&active);
                thread::spawn(move || {
                    loop {
                        let job = receiver
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .recv();
                        let Ok(job) = job else {
                            break;
                        };
                        let source_revision_id = match &job.pending {
                            PendingPersistence::Encoded(pending) => {
                                pending.source_revision.revision_id.clone()
                            }
                            PendingPersistence::Staged { pending, .. } => {
                                pending.source_revision.revision_id.clone()
                            }
                        };
                        let published = match job.pending {
                            PendingPersistence::Encoded(pending) => cache.publish(pending),
                            PendingPersistence::Staged { pending, _owner } => {
                                let published = cache.publish_staged(pending);
                                drop(_owner);
                                published
                            }
                        };
                        match published {
                            Ok(publication) => {
                                let crate::cache::ArtifactLocation::Managed(path) =
                                    &publication.artifact.location
                                else {
                                    unreachable!("persistent cache returns a managed artifact")
                                };
                                let path = path.clone();
                                let mut lease = publication.lease;
                                let transition = loop {
                                    match registry.transition_to_managed_file(
                                        &job.resource_id,
                                        &path,
                                        &mut lease,
                                    ) {
                                        Ok(true) => break Ok(()),
                                        Ok(false) => thread::sleep(Duration::from_millis(2)),
                                        Err(error) => break Err(error),
                                    }
                                };
                                let completion = match transition {
                                    Ok(()) => PersistenceCompletion::Persisted {
                                        resource_id: job.resource_id.clone(),
                                        path,
                                    },
                                    Err(error) => {
                                        let message = error.to_string();
                                        record_failure(
                                            &failures,
                                            source_revision_id.clone(),
                                            message.clone(),
                                        );
                                        PersistenceCompletion::Failed {
                                            resource_id: job.resource_id.clone(),
                                            message,
                                        }
                                    }
                                };
                                notify_active_subscribers(&active, &completion);
                                remove_active_publication(&active, &job.resource_id);
                                let _ = job.completion.send(completion);
                            }
                            Err(error) => {
                                let message = error.to_string();
                                record_failure(&failures, source_revision_id, message.clone());
                                update_active_publication(
                                    &active,
                                    &job.resource_id,
                                    PathBuf::new(),
                                    PersistenceStatus::SkippedBackpressure,
                                );
                                let completion = PersistenceCompletion::Failed {
                                    resource_id: job.resource_id,
                                    message,
                                };
                                notify_active_subscribers(&active, &completion);
                                let _ = job.completion.send(completion);
                            }
                        }
                        pending_bytes.fetch_sub(job.memory_bytes, Ordering::AcqRel);
                    }
                })
            })
            .collect();
        Self {
            registry,
            cache,
            sender: Some(sender),
            workers,
            failures,
            active,
            pending_bytes,
            max_pending_bytes,
            staging_parent,
            staging: Mutex::new(None),
        }
    }

    pub(crate) fn adopt_staged_file(
        &self,
        staged: tempfile::TempPath,
        extension: &str,
    ) -> Result<Arc<OwnedStagedFile>, MediaError> {
        let mut staging = self
            .staging
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if staging.is_none() {
            *staging = Some(
                tempfile::Builder::new()
                    .prefix(".resource-stage-")
                    .tempdir_in(&self.staging_parent)?,
            );
        }
        let directory = staging.as_ref().expect("resource staging was initialized");
        let destination = tempfile::Builder::new()
            .suffix(&format!(".{extension}"))
            .tempfile_in(directory.path())?
            .into_temp_path();
        std::fs::remove_file(&destination)?;
        std::fs::rename(&staged, &destination)?;
        let path = destination
            .keep()
            .map_err(|error| MediaError::Io(error.error))?;
        Ok(Arc::new(OwnedStagedFile::new(path)))
    }

    pub fn publish(
        &self,
        artifact: ProducedArtifact,
        persist: bool,
    ) -> Result<PublishedArtifact, MediaError> {
        let ProducedArtifact {
            source_revision,
            variant,
            mut facts,
            cache_generation,
            payload,
        } = artifact;
        crate::media_source::bind_facts(&mut facts, &source_revision)?;
        let origin = facts.source.origin;
        let actual_dimensions = facts.display_dimensions.0;
        let active_artifact = MediaArtifact {
            artifact_id: String::new(),
            source_revision: source_revision.clone(),
            variant: variant.clone(),
            facts: facts.clone(),
            byte_size: match &payload {
                ProducedPayload::Encoded { bytes, .. } => bytes.len() as u64,
                ProducedPayload::StagedFile { owner, .. } => {
                    fs::metadata(owner.path()).map_or(0, |metadata| metadata.len())
                }
                ProducedPayload::OriginalFile { path, .. } => {
                    fs::metadata(path).map_or(0, |metadata| metadata.len())
                }
            },
            media_type: match &payload {
                ProducedPayload::OriginalFile { media_type, .. }
                | ProducedPayload::Encoded { media_type, .. }
                | ProducedPayload::StagedFile { media_type, .. } => media_type.clone(),
            },
            location: match &payload {
                ProducedPayload::StagedFile { owner, .. } => {
                    ArtifactLocation::Managed(owner.path().to_owned())
                }
                ProducedPayload::OriginalFile { path, .. } => {
                    ArtifactLocation::Managed(path.clone())
                }
                ProducedPayload::Encoded { .. } => ArtifactLocation::Managed(PathBuf::new()),
            },
        };
        let (resource, pending, memory_bytes) = match payload {
            ProducedPayload::OriginalFile { path, media_type } => (
                self.registry
                    .register_file(&path, media_type, actual_dimensions, origin)?,
                None,
                0,
            ),
            ProducedPayload::Encoded {
                bytes,
                media_type,
                extension,
            } => {
                let resource = self.registry.register_encoded(
                    Arc::clone(&bytes),
                    media_type.clone(),
                    actual_dimensions,
                    origin,
                )?;
                let memory_bytes = bytes.len();
                let pending = persist.then_some(PendingPersistence::Encoded(PendingArtifact {
                    source_revision,
                    variant,
                    facts,
                    media_type,
                    extension,
                    bytes,
                    cache_generation,
                }));
                (resource, pending, memory_bytes)
            }
            ProducedPayload::StagedFile {
                owner,
                media_type,
                extension,
            } => {
                let resource = self.registry.register_owned_staged_file(
                    Arc::clone(&owner),
                    media_type.clone(),
                    actual_dimensions,
                    origin,
                )?;
                let pending = persist.then_some(PendingPersistence::Staged {
                    pending: crate::cache::PendingStagedArtifact {
                        source_revision,
                        variant,
                        facts,
                        media_type,
                        extension,
                        staged_path: owner.path().to_owned(),
                        cache_generation,
                    },
                    _owner: owner,
                });
                (resource, pending, 0)
            }
        };
        // The immutable resource URL is sufficient for the UI. Deriving the
        // managed path hashes the full payload, so leave that work to the
        // persistence worker rather than blocking resource readiness.
        let managed_path = None;
        let mut completion = None;
        let persistence = match pending {
            None => PersistenceStatus::NotRequested,
            Some(pending) => {
                if !reserve_pending_bytes(&self.pending_bytes, self.max_pending_bytes, memory_bytes)
                {
                    PersistenceStatus::SkippedBackpressure
                } else {
                    let (completion_sender, completion_receiver) = mpsc::channel();
                    let resource_id = resource.descriptor.resource_id.clone();
                    {
                        let mut active = self
                            .active
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        reap_active_publications(&mut active, &self.registry);
                        active
                            .entry(active_artifact.source_revision.revision_id.clone())
                            .or_default()
                            .push(ActivePublication {
                                artifact: active_artifact,
                                resource: resource.descriptor.clone(),
                                persistence: PersistenceStatus::Scheduled,
                                cache_generation,
                                subscribers: Vec::new(),
                            });
                    }
                    let job = PersistenceJob {
                        pending,
                        resource_id: resource_id.clone(),
                        memory_bytes,
                        completion: completion_sender,
                    };
                    match self
                        .sender
                        .as_ref()
                        .expect("publisher sender exists until drop")
                        .try_send(job)
                    {
                        Ok(()) => {
                            completion = Some(completion_receiver);
                            PersistenceStatus::Scheduled
                        }
                        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                            self.pending_bytes.fetch_sub(memory_bytes, Ordering::AcqRel);
                            remove_active_publication(&self.active, &resource_id);
                            PersistenceStatus::SkippedBackpressure
                        }
                    }
                }
            }
        };
        Ok(PublishedArtifact {
            resource,
            managed_path,
            persistence,
            completion,
        })
    }

    pub fn lookup_active(
        &self,
        request: &CacheRequest,
    ) -> Result<Option<ActivePublicationHit>, MediaError> {
        let current_generation = self.cache.generation()?;
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reap_active_publications(&mut active, &self.registry);
        let Some(publications) = active.get_mut(&request.source_revision.revision_id) else {
            return Ok(None);
        };
        let selected = publications
            .iter()
            .enumerate()
            .filter(|(_, publication)| publication.cache_generation == current_generation)
            .filter_map(|(index, publication)| {
                satisfies(&publication.artifact, request).map(|satisfaction| (index, satisfaction))
            })
            .min_by_key(|(index, satisfaction)| {
                candidate_rank(&publications[*index].artifact, *satisfaction)
            });
        let Some((index, satisfaction)) = selected else {
            return Ok(None);
        };
        let publication = &mut publications[index];
        if !self
            .registry
            .refresh_publication_grace(&publication.resource.resource_id)
        {
            return Ok(None);
        }
        let completion = if publication.persistence == PersistenceStatus::Scheduled {
            let (sender, receiver) = mpsc::channel();
            publication.subscribers.push(sender);
            Some(receiver)
        } else {
            None
        };
        Ok(Some(ActivePublicationHit {
            artifact: publication.artifact.clone(),
            resource: publication.resource.clone(),
            satisfaction,
            persistence: publication.persistence,
            completion,
        }))
    }

    pub fn take_failures(&self) -> Vec<PersistenceFailure> {
        std::mem::take(
            &mut *self
                .failures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub fn registry(&self) -> &ResourceRegistry {
        &self.registry
    }

    #[cfg(test)]
    fn active_record_count(&self) -> usize {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reap_active_publications(&mut active, &self.registry);
        active.values().map(Vec::len).sum()
    }
}

fn reap_active_publications(
    active: &mut HashMap<String, Vec<ActivePublication>>,
    registry: &ResourceRegistry,
) {
    active.retain(|_, publications| {
        publications.retain(|publication| registry.contains(&publication.resource.resource_id));
        !publications.is_empty()
    });
}

fn notify_active_subscribers(
    active: &Mutex<HashMap<String, Vec<ActivePublication>>>,
    completion: &PersistenceCompletion,
) {
    let resource_id = match completion {
        PersistenceCompletion::Persisted { resource_id, .. }
        | PersistenceCompletion::Failed { resource_id, .. } => resource_id,
    };
    let mut active = active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for publications in active.values_mut() {
        if let Some(publication) = publications
            .iter_mut()
            .find(|publication| &publication.resource.resource_id == resource_id)
        {
            for subscriber in publication.subscribers.drain(..) {
                let _ = subscriber.send(completion.clone());
            }
            return;
        }
    }
}

fn update_active_publication(
    active: &Mutex<HashMap<String, Vec<ActivePublication>>>,
    resource_id: &str,
    path: PathBuf,
    persistence: PersistenceStatus,
) {
    let mut active = active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for publications in active.values_mut() {
        if let Some(publication) = publications
            .iter_mut()
            .find(|publication| publication.resource.resource_id == resource_id)
        {
            if !path.as_os_str().is_empty() {
                publication.artifact.location = ArtifactLocation::Managed(path);
            }
            publication.persistence = persistence;
            return;
        }
    }
}

fn remove_active_publication(
    active: &Mutex<HashMap<String, Vec<ActivePublication>>>,
    resource_id: &str,
) {
    let mut active = active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    active.retain(|_, publications| {
        publications.retain(|publication| publication.resource.resource_id != resource_id);
        !publications.is_empty()
    });
}

fn record_failure(
    failures: &Mutex<Vec<PersistenceFailure>>,
    source_revision_id: String,
    message: String,
) {
    let mut failures = failures
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if failures.len() == 128 {
        failures.remove(0);
    }
    failures.push(PersistenceFailure {
        source_revision_id,
        message,
    });
}

fn reserve_pending_bytes(counter: &AtomicUsize, maximum: usize, additional: usize) -> bool {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(additional) else {
            return false;
        };
        if next > maximum {
            return false;
        }
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
}

impl Drop for ArtifactPublisher {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{
        ArtifactLocation, ArtifactPresentation, ArtifactRequirement, CacheColorState, CacheRequest,
        ColorRequirement, DetailRequirement, DiskMediaCache, MEDIA_CACHE_POLICY_REVISION,
        OrientationRequirement, OrientationState, PresentationRequirement, SharpeningState,
    };
    use image::{DynamicImage, ImageFormat};
    use std::{
        io::Cursor,
        sync::{Condvar, Mutex},
        thread,
        time::Duration,
    };

    fn source(directory: &Path) -> SourceRevision {
        let path = directory.join("source.jpg");
        DynamicImage::new_rgb8(16, 8).save(&path).unwrap();
        oxy_fs::observe_source_revision(&path).unwrap()
    }

    fn variant(_origin: ImageOrigin) -> VariantIdentity {
        VariantIdentity {
            presentation: ArtifactPresentation {
                geometry: None,
                orientation: OrientationState::Applied,
                color: CacheColorState::Srgb,
                sharpening: SharpeningState::None,
            },
            policy_revision: MEDIA_CACHE_POLICY_REVISION,
            target: "test".into(),
        }
    }

    fn jpeg() -> Arc<[u8]> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(16, 8)
            .write_to(&mut bytes, ImageFormat::Jpeg)
            .unwrap();
        Arc::from(bytes.into_inner())
    }

    #[test]
    fn persistence_memory_reservation_is_bounded() {
        let counter = AtomicUsize::new(0);
        assert!(reserve_pending_bytes(&counter, 10, 6));
        assert!(!reserve_pending_bytes(&counter, 10, 5));
        counter.fetch_sub(6, Ordering::AcqRel);
        assert!(reserve_pending_bytes(&counter, 10, 10));
    }

    #[test]
    fn resource_ids_are_unique_across_registry_instances_and_process_namespaced() {
        let first_registry = ResourceRegistry::new(ResourceRegistryLimits::new(1, 16));
        let second_registry = ResourceRegistry::new(ResourceRegistryLimits::new(1, 16));
        let register = |registry: &ResourceRegistry| {
            registry
                .register_encoded(
                    Arc::from([1_u8]),
                    "image/jpeg",
                    PixelDimensions {
                        width: 1,
                        height: 1,
                    },
                    ImageOrigin::EmbeddedPreview,
                )
                .unwrap()
        };
        let first = register(&first_registry);
        let second = register(&second_registry);
        assert_ne!(first.descriptor.resource_id, second.descriptor.resource_id);
        assert!(first.descriptor.resource_id.starts_with("resource-"));
        assert!(first.descriptor.resource_id.matches('-').count() >= 3);
    }

    #[test]
    fn original_jpeg_is_registered_without_copying_or_exposing_unregistered_paths() {
        let directory = tempfile::tempdir().unwrap();
        let source = source(directory.path());
        let registry = ResourceRegistry::new(ResourceRegistryLimits::new(2, 1024));
        let handle = registry
            .register_file(
                &source.canonical_path,
                "image/jpeg",
                PixelDimensions {
                    width: 16,
                    height: 8,
                },
                ImageOrigin::PrimaryImage,
            )
            .unwrap();
        assert!(registry.resolve("../../source.jpg").is_none());
        let resolved = registry.resolve(&handle.descriptor.resource_id).unwrap();
        assert!(
            matches!(resolved.payload, ResourcePayload::File(ref path) if path == &source.canonical_path)
        );
    }

    #[test]
    fn file_resource_never_serves_replacement_bytes_under_the_same_id() {
        let directory = tempfile::tempdir().unwrap();
        let source = source(directory.path());
        let registry = ResourceRegistry::new(ResourceRegistryLimits::new(2, 1024));
        let handle = registry
            .register_file(
                &source.canonical_path,
                "image/jpeg",
                PixelDimensions {
                    width: 16,
                    height: 8,
                },
                ImageOrigin::PrimaryImage,
            )
            .unwrap();
        fs::write(&source.canonical_path, b"replacement").unwrap();
        assert!(registry.resolve(&handle.descriptor.resource_id).is_none());
        assert!(!registry.contains(&handle.descriptor.resource_id));
    }

    #[test]
    fn managed_file_resource_renews_the_disk_lease_across_clear() {
        let directory = tempfile::tempdir().unwrap();
        let source = source(directory.path());
        // The shared publisher canonicalizes its parent, while maintenance
        // receives the configured path (C:\... vs \\?\C:\... on Windows).
        let cache = DiskMediaCache::new(directory.path().canonicalize().unwrap(), 8).unwrap();
        let bytes = jpeg();
        cache
            .publish(PendingArtifact {
                source_revision: source.clone(),
                variant: variant(ImageOrigin::EmbeddedPreview),
                facts: crate::media_source::test_facts(
                    ImageOrigin::EmbeddedPreview,
                    PixelDimensions {
                        width: 16,
                        height: 8,
                    },
                    false,
                ),
                media_type: "image/jpeg".into(),
                extension: "jpg".into(),
                bytes,
                cache_generation: cache.generation().unwrap(),
            })
            .unwrap();
        let request = CacheRequest {
            source_revision: source,
            detail: DetailRequirement::Display { min_long_edge: 16 },
            artifact: ArtifactRequirement::AnyDisplay,
            presentation: PresentationRequirement {
                orientation: OrientationRequirement::DisplayCorrect,
                color: ColorRequirement::Srgb,
                sharpening: SharpeningState::None,
            },
            policy_revision: MEDIA_CACHE_POLICY_REVISION,
            allow_interim: false,
        };
        let hit = cache.lookup(&request).unwrap().unwrap();
        let ArtifactLocation::Managed(path) = &hit.artifact.location else {
            unreachable!()
        };
        let path = path.clone();
        let registry = ResourceRegistry::new(ResourceRegistryLimits::new(2, 1024 * 1024));
        let handle = registry
            .register_file_with_lease(
                &path,
                "image/jpeg",
                hit.artifact.facts.display_dimensions.0,
                hit.artifact.facts.source.origin,
                Some(hit.lease),
            )
            .unwrap();
        assert!(registry.renew(&handle.descriptor.resource_id));
        let maintenance = DiskMediaCache::new(directory.path(), 8).unwrap();
        maintenance.clear().unwrap();
        assert!(cache.lookup(&request).unwrap().is_none());
        let read = registry.resolve(&handle.descriptor.resource_id).unwrap();
        assert!(!registry.materialize(&read).unwrap().is_empty());
        assert!(path.is_file());
    }

    #[test]
    fn protocol_materialization_rejects_a_file_larger_than_its_response_budget() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.jpg");
        fs::write(&path, [0_u8; 9]).unwrap();
        let registry = ResourceRegistry::new(ResourceRegistryLimits {
            max_materialized_bytes: 8,
            ..ResourceRegistryLimits::new(2, 8)
        });
        let handle = registry
            .register_file(
                &path,
                "image/jpeg",
                PixelDimensions {
                    width: 1,
                    height: 1,
                },
                ImageOrigin::PrimaryImage,
            )
            .unwrap();
        let read = registry.resolve(&handle.descriptor.resource_id).unwrap();
        assert!(matches!(
            registry.materialize(&read),
            Err(MediaError::ResourceBudgetExhausted { .. })
        ));
    }

    #[test]
    fn registry_enforces_memory_budget_without_evicting_leased_resource() {
        let registry = ResourceRegistry::new(ResourceRegistryLimits::new(1, 4));
        let first = registry
            .register_encoded(
                Arc::from([1_u8, 2, 3, 4]),
                "image/jpeg",
                PixelDimensions {
                    width: 1,
                    height: 1,
                },
                ImageOrigin::EmbeddedPreview,
            )
            .unwrap();
        assert!(matches!(
            registry.register_encoded(
                Arc::from([5_u8]),
                "image/jpeg",
                PixelDimensions {
                    width: 1,
                    height: 1
                },
                ImageOrigin::EmbeddedPreview,
            ),
            Err(MediaError::ResourceBudgetExhausted { .. })
        ));
        registry.release(&first.descriptor.resource_id);
        drop(first);
        assert!(
            registry
                .register_encoded(
                    Arc::from([5_u8]),
                    "image/jpeg",
                    PixelDimensions {
                        width: 1,
                        height: 1
                    },
                    ImageOrigin::EmbeddedPreview,
                )
                .is_ok()
        );
    }

    struct BlockingCache {
        gate: Arc<(Mutex<bool>, Condvar)>,
        started: Arc<(Mutex<bool>, Condvar)>,
        seen_generation: Arc<AtomicU64>,
        generation: Arc<AtomicU64>,
    }

    impl MediaCache for BlockingCache {
        fn generation(&self) -> Result<u64, MediaError> {
            Ok(self.generation.load(Ordering::Acquire))
        }

        fn lookup_or_generation(
            &self,
            _request: &CacheRequest,
        ) -> Result<crate::cache::CacheLookup, MediaError> {
            Ok(crate::cache::CacheLookup::Generate {
                cache_generation: 7,
            })
        }

        fn planned_location(
            &self,
            _pending: &PendingArtifact,
        ) -> Result<Option<PathBuf>, MediaError> {
            Ok(Some(PathBuf::from("planned.jpg")))
        }

        fn publish(
            &self,
            pending: PendingArtifact,
        ) -> Result<crate::cache::CachePublication, MediaError> {
            self.seen_generation
                .store(pending.cache_generation, Ordering::Release);
            let (started, notify) = &*self.started;
            *started.lock().unwrap() = true;
            notify.notify_all();
            let (gate, notify) = &*self.gate;
            let mut open = gate.lock().unwrap();
            while !*open {
                open = notify.wait(open).unwrap();
            }
            Err(MediaError::CacheArtifact(format!(
                "intentional failure for {}",
                pending.source_revision.revision_id
            )))
        }

        fn clear(&self) -> Result<(), MediaError> {
            self.generation.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    struct MissingTransitionCache;

    impl MediaCache for MissingTransitionCache {
        fn generation(&self) -> Result<u64, MediaError> {
            Ok(7)
        }

        fn lookup_or_generation(
            &self,
            _request: &CacheRequest,
        ) -> Result<crate::cache::CacheLookup, MediaError> {
            Ok(crate::cache::CacheLookup::Generate {
                cache_generation: 7,
            })
        }

        fn planned_location(
            &self,
            _pending: &PendingArtifact,
        ) -> Result<Option<PathBuf>, MediaError> {
            Ok(None)
        }

        fn publish(
            &self,
            pending: PendingArtifact,
        ) -> Result<crate::cache::CachePublication, MediaError> {
            Ok(crate::cache::CachePublication {
                artifact: crate::cache::MediaArtifact {
                    artifact_id: "missing".into(),
                    source_revision: pending.source_revision,
                    variant: pending.variant,
                    facts: pending.facts.clone(),
                    byte_size: pending.bytes.len() as u64,
                    media_type: pending.media_type,
                    location: ArtifactLocation::Managed(PathBuf::from("missing-managed.jpg")),
                },
                lease: None,
            })
        }

        fn clear(&self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    #[test]
    fn failed_registry_transition_never_reports_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let source = source(directory.path());
        let publisher = ArtifactPublisher::new(
            ResourceRegistry::new(ResourceRegistryLimits::new(4, 1024 * 1024)),
            Arc::new(MissingTransitionCache),
            1,
            1024 * 1024,
            1,
        );
        let published = publisher
            .publish(
                ProducedArtifact {
                    source_revision: source,
                    variant: variant(ImageOrigin::EmbeddedPreview),
                    facts: crate::media_source::test_facts(
                        ImageOrigin::EmbeddedPreview,
                        PixelDimensions {
                            width: 16,
                            height: 8,
                        },
                        false,
                    ),
                    cache_generation: 7,
                    payload: ProducedPayload::Encoded {
                        bytes: jpeg(),
                        media_type: "image/jpeg".into(),
                        extension: "jpg".into(),
                    },
                },
                true,
            )
            .unwrap();
        let resource_id = published.resource.descriptor.resource_id.clone();
        assert!(matches!(
            published
                .completion
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .unwrap(),
            PersistenceCompletion::Failed { .. }
        ));
        assert!(publisher.registry().resolve(&resource_id).is_some());
    }

    #[test]
    fn adopted_ui_stage_survives_cache_clear() {
        let directory = tempfile::tempdir().unwrap();
        let cache = Arc::new(DiskMediaCache::new(directory.path(), 8).unwrap());
        let backend_staging = cache.root().join(".backend-tmp");
        fs::create_dir(&backend_staging).unwrap();
        let staged = backend_staging.join("native-output.jpg");
        fs::write(&staged, jpeg().as_ref()).unwrap();
        let publisher = ArtifactPublisher::new_with_staging_parent(
            ResourceRegistry::new(ResourceRegistryLimits::new(4, 1024 * 1024)),
            cache.clone(),
            1,
            1024 * 1024,
            1,
            directory.path().to_owned(),
        );

        let staged_temp = tempfile::NamedTempFile::new_in(&backend_staging).unwrap();
        fs::copy(&staged, staged_temp.path()).unwrap();
        let adopted = publisher
            .adopt_staged_file(staged_temp.into_temp_path(), "jpg")
            .unwrap();
        assert!(!adopted.path().starts_with(cache.root()));
        cache.clear().unwrap();
        assert!(adopted.path().is_file());
        assert!(!staged.exists());
    }

    #[test]
    fn registry_failure_drops_the_adopted_staged_owner() {
        let directory = tempfile::tempdir().unwrap();
        let registry = ResourceRegistry::new(ResourceRegistryLimits::new(1, 1024 * 1024));
        let _occupied = registry
            .register_encoded(
                Arc::from([1_u8]),
                "image/jpeg",
                PixelDimensions {
                    width: 1,
                    height: 1,
                },
                ImageOrigin::EmbeddedPreview,
            )
            .unwrap();
        let cache = Arc::new(DiskMediaCache::new(directory.path(), 8).unwrap());
        let publisher = ArtifactPublisher::new_with_staging_parent(
            registry,
            cache.clone(),
            1,
            1024 * 1024,
            1,
            directory.path().to_owned(),
        );
        let backend_staging = cache.root().join(".backend-tmp");
        fs::create_dir(&backend_staging).unwrap();
        let staged = tempfile::Builder::new()
            .suffix(".jpg")
            .tempfile_in(&backend_staging)
            .unwrap();
        fs::write(staged.path(), jpeg().as_ref()).unwrap();
        let owner = publisher
            .adopt_staged_file(staged.into_temp_path(), "jpg")
            .unwrap();
        let adopted_path = owner.path().to_owned();
        let result = publisher.publish(
            ProducedArtifact {
                source_revision: source(directory.path()),
                variant: variant(ImageOrigin::RawSensor),
                facts: crate::media_source::test_facts(
                    ImageOrigin::RawSensor,
                    PixelDimensions {
                        width: 16,
                        height: 8,
                    },
                    true,
                ),
                cache_generation: 0,
                payload: ProducedPayload::StagedFile {
                    owner: Arc::clone(&owner),
                    media_type: "image/jpeg".into(),
                    extension: "jpg".into(),
                },
            },
            true,
        );
        assert!(matches!(
            result,
            Err(MediaError::ResourceBudgetExhausted { .. })
        ));
        drop(owner);
        assert!(!adopted_path.exists());
    }

    #[test]
    fn successful_persistence_transitions_resource_to_managed_file() {
        let directory = tempfile::tempdir().unwrap();
        let source = source(directory.path());
        let cache = Arc::new(DiskMediaCache::new(directory.path(), 8).unwrap());
        let staging = cache.root().join(".backend-tmp");
        fs::create_dir(&staging).unwrap();
        let staged = staging.join("native-output.jpg");
        fs::write(&staged, jpeg().as_ref()).unwrap();
        let registry = ResourceRegistry::new(ResourceRegistryLimits::new(4, 1024 * 1024));
        let publisher = ArtifactPublisher::new(registry.clone(), cache, 1, 1024 * 1024, 1);
        let published = publisher
            .publish(
                ProducedArtifact {
                    source_revision: source,
                    variant: variant(ImageOrigin::RawSensor),
                    facts: crate::media_source::test_facts(
                        ImageOrigin::RawSensor,
                        PixelDimensions {
                            width: 16,
                            height: 8,
                        },
                        true,
                    ),
                    cache_generation: 0,
                    payload: ProducedPayload::StagedFile {
                        owner: Arc::new(OwnedStagedFile::new(staged.clone())),
                        media_type: "image/jpeg".into(),
                        extension: "jpg".into(),
                    },
                },
                true,
            )
            .unwrap();
        let resource_id = published.resource.descriptor.resource_id.clone();
        let completion = published.completion.unwrap();
        drop(published.resource);
        let PersistenceCompletion::Persisted { path, .. } =
            completion.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("persistence unexpectedly failed")
        };
        assert!(path.is_file());
        assert!(!staged.exists());
        let read = registry.resolve(&resource_id).unwrap();
        assert!(matches!(
            read.payload,
            ResourcePayload::File(ref current)
                if current == &path.canonicalize().unwrap()
        ));
    }

    #[test]
    fn staged_native_file_is_ui_ready_before_delayed_persistence() {
        let directory = tempfile::tempdir().unwrap();
        let source = source(directory.path());
        let staged = directory.path().join("native-output.jpg");
        fs::write(&staged, jpeg().as_ref()).unwrap();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let cache = Arc::new(BlockingCache {
            gate: Arc::clone(&gate),
            started: Arc::clone(&started),
            seen_generation: Arc::new(AtomicU64::new(u64::MAX)),
            generation: Arc::new(AtomicU64::new(7)),
        });
        let publisher = ArtifactPublisher::new(
            ResourceRegistry::new(ResourceRegistryLimits::new(4, 1024 * 1024)),
            cache,
            1,
            1024 * 1024,
            1,
        );
        let published = publisher
            .publish(
                ProducedArtifact {
                    source_revision: source,
                    variant: variant(ImageOrigin::RawSensor),
                    facts: crate::media_source::test_facts(
                        ImageOrigin::RawSensor,
                        PixelDimensions {
                            width: 16,
                            height: 8,
                        },
                        true,
                    ),
                    cache_generation: 7,
                    payload: ProducedPayload::StagedFile {
                        owner: Arc::new(OwnedStagedFile::new(staged.clone())),
                        media_type: "image/jpeg".into(),
                        extension: "jpg".into(),
                    },
                },
                true,
            )
            .unwrap();
        let read = publisher
            .registry()
            .resolve(&published.resource.descriptor.resource_id)
            .unwrap();
        assert!(!publisher.registry().materialize(&read).unwrap().is_empty());
        assert!(staged.is_file());
        let (started_lock, notify) = &*started;
        let mut did_start = started_lock.lock().unwrap();
        while !*did_start {
            did_start = notify.wait(did_start).unwrap();
        }
        drop(did_start);
        let (gate_lock, notify) = &*gate;
        *gate_lock.lock().unwrap() = true;
        notify.notify_all();
        drop(read);
        drop(published);
        drop(publisher);
    }

    #[test]
    fn compatible_cross_level_lookup_reuses_pending_publication() {
        let directory = tempfile::tempdir().unwrap();
        let source = source(directory.path());
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let cache = Arc::new(BlockingCache {
            gate: Arc::clone(&gate),
            started,
            seen_generation: Arc::new(AtomicU64::new(u64::MAX)),
            generation: Arc::new(AtomicU64::new(7)),
        });
        let publisher = ArtifactPublisher::new(
            ResourceRegistry::new(ResourceRegistryLimits::new(4, 1024 * 1024)),
            cache,
            1,
            1024 * 1024,
            1,
        );
        let published = publisher
            .publish(
                ProducedArtifact {
                    source_revision: source.clone(),
                    variant: variant(ImageOrigin::EmbeddedPreview),
                    facts: crate::media_source::test_facts(
                        ImageOrigin::EmbeddedPreview,
                        PixelDimensions {
                            width: 16,
                            height: 8,
                        },
                        false,
                    ),
                    cache_generation: 7,
                    payload: ProducedPayload::Encoded {
                        bytes: jpeg(),
                        media_type: "image/jpeg".into(),
                        extension: "jpg".into(),
                    },
                },
                true,
            )
            .unwrap();
        let request = CacheRequest {
            source_revision: source,
            detail: DetailRequirement::Display { min_long_edge: 8 },
            artifact: ArtifactRequirement::AnyDisplay,
            presentation: PresentationRequirement {
                orientation: OrientationRequirement::DisplayCorrect,
                color: ColorRequirement::Any,
                sharpening: SharpeningState::None,
            },
            policy_revision: MEDIA_CACHE_POLICY_REVISION,
            allow_interim: false,
        };
        let hit = publisher.lookup_active(&request).unwrap().unwrap();
        assert_eq!(hit.satisfaction, Satisfaction::Satisfied);
        assert_eq!(
            hit.resource.resource_id,
            published.resource.descriptor.resource_id
        );
        assert!(hit.completion.is_some());
        let (gate_lock, notify) = &*gate;
        *gate_lock.lock().unwrap() = true;
        notify.notify_all();
        drop(hit);
        drop(published);
        drop(publisher);
    }

    #[test]
    fn clear_invalidates_active_lookup_without_revoking_existing_resource() {
        let directory = tempfile::tempdir().unwrap();
        let source = source(directory.path());
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let cache = Arc::new(BlockingCache {
            gate: Arc::clone(&gate),
            started,
            seen_generation: Arc::new(AtomicU64::new(u64::MAX)),
            generation: Arc::new(AtomicU64::new(7)),
        });
        let publisher = ArtifactPublisher::new(
            ResourceRegistry::new(ResourceRegistryLimits::new(4, 1024 * 1024)),
            cache.clone(),
            1,
            1024 * 1024,
            1,
        );
        let published = publisher
            .publish(
                ProducedArtifact {
                    source_revision: source.clone(),
                    variant: variant(ImageOrigin::EmbeddedPreview),
                    facts: crate::media_source::test_facts(
                        ImageOrigin::EmbeddedPreview,
                        PixelDimensions {
                            width: 16,
                            height: 8,
                        },
                        false,
                    ),
                    cache_generation: 7,
                    payload: ProducedPayload::Encoded {
                        bytes: jpeg(),
                        media_type: "image/jpeg".into(),
                        extension: "jpg".into(),
                    },
                },
                true,
            )
            .unwrap();
        let request = CacheRequest {
            source_revision: source,
            detail: DetailRequirement::Display { min_long_edge: 8 },
            artifact: ArtifactRequirement::AnyDisplay,
            presentation: PresentationRequirement {
                orientation: OrientationRequirement::DisplayCorrect,
                color: ColorRequirement::Any,
                sharpening: SharpeningState::None,
            },
            policy_revision: MEDIA_CACHE_POLICY_REVISION,
            allow_interim: false,
        };
        assert!(publisher.lookup_active(&request).unwrap().is_some());
        cache.clear().unwrap();
        assert!(publisher.lookup_active(&request).unwrap().is_none());
        let resource_id = &published.resource.descriptor.resource_id;
        assert!(publisher.registry().resolve(resource_id).is_some());
        let (gate_lock, notify) = &*gate;
        *gate_lock.lock().unwrap() = true;
        notify.notify_all();
        drop(published);
        drop(publisher);
    }

    #[test]
    fn completed_active_metadata_is_bounded_by_registry_capacity() {
        let directory = tempfile::tempdir().unwrap();
        let gate = Arc::new((Mutex::new(true), Condvar::new()));
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let cache = Arc::new(BlockingCache {
            gate,
            started,
            seen_generation: Arc::new(AtomicU64::new(u64::MAX)),
            generation: Arc::new(AtomicU64::new(7)),
        });
        let publisher = ArtifactPublisher::new(
            ResourceRegistry::new(ResourceRegistryLimits::new(4, 1024 * 1024)),
            cache,
            1,
            1024 * 1024,
            1,
        );
        for index in 0..12 {
            let source = source(directory.path());
            let mut source = source;
            source.revision_id = format!("{index:064x}");
            let published = publisher
                .publish(
                    ProducedArtifact {
                        source_revision: source,
                        variant: variant(ImageOrigin::EmbeddedPreview),
                        facts: crate::media_source::test_facts(
                            ImageOrigin::EmbeddedPreview,
                            PixelDimensions {
                                width: 16,
                                height: 8,
                            },
                            false,
                        ),
                        cache_generation: 7,
                        payload: ProducedPayload::Encoded {
                            bytes: jpeg(),
                            media_type: "image/jpeg".into(),
                            extension: "jpg".into(),
                        },
                    },
                    true,
                )
                .unwrap();
            publisher
                .registry()
                .release(&published.resource.descriptor.resource_id);
            drop(published);
            thread::sleep(Duration::from_millis(2));
        }
        assert!(publisher.active_record_count() <= 4);
    }

    #[test]
    fn embedded_jpeg_is_ui_ready_before_persistence_and_failure_does_not_revoke_it() {
        let directory = tempfile::tempdir().unwrap();
        let source = source(directory.path());
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let seen_generation = Arc::new(AtomicU64::new(u64::MAX));
        let cache = Arc::new(BlockingCache {
            gate: Arc::clone(&gate),
            started: Arc::clone(&started),
            seen_generation: Arc::clone(&seen_generation),
            generation: Arc::new(AtomicU64::new(7)),
        });
        let publisher = ArtifactPublisher::new(
            ResourceRegistry::new(ResourceRegistryLimits::new(4, 1024 * 1024)),
            cache,
            1,
            1024 * 1024,
            1,
        );
        let bytes = jpeg();
        let published = publisher
            .publish(
                ProducedArtifact {
                    source_revision: source,
                    variant: variant(ImageOrigin::EmbeddedPreview),
                    facts: crate::media_source::test_facts(
                        ImageOrigin::EmbeddedPreview,
                        PixelDimensions {
                            width: 16,
                            height: 8,
                        },
                        false,
                    ),
                    cache_generation: 7,
                    payload: ProducedPayload::Encoded {
                        bytes: Arc::clone(&bytes),
                        media_type: "image/jpeg".into(),
                        extension: "jpg".into(),
                    },
                },
                true,
            )
            .unwrap();
        assert_eq!(published.persistence, PersistenceStatus::Scheduled);
        let resource_id = published.resource.descriptor.resource_id.clone();
        let resolved = publisher.registry().resolve(&resource_id).unwrap();
        assert!(
            matches!(resolved.payload, ResourcePayload::Encoded(ref resolved) if Arc::ptr_eq(resolved, &bytes))
        );
        // Dropping the Rust publication handle does not cancel the cache
        // consumer or the expiring UI lease.
        drop(published);
        let (started_lock, notify) = &*started;
        let mut did_start = started_lock.lock().unwrap();
        while !*did_start {
            did_start = notify.wait(did_start).unwrap();
        }
        drop(did_start);
        assert_eq!(seen_generation.load(Ordering::Acquire), 7);
        let (gate_lock, notify) = &*gate;
        *gate_lock.lock().unwrap() = true;
        notify.notify_all();
        let mut failures = Vec::new();
        for _ in 0..100 {
            failures = publisher.take_failures();
            if !failures.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(failures.len(), 1);
        assert!(publisher.registry().resolve(&resource_id).is_some());
        drop(resolved);
        drop(publisher);
    }
}
