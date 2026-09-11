use super::model::{
    ArtifactLocation, CacheRequest, DisplayDimensions, MediaArtifact, Satisfaction, SourceRevision,
    VariantIdentity, candidate_rank, satisfies,
};
use crate::MediaError;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use tempfile::NamedTempFile;

const CACHE_FOLDER: &str = "media-cache-v2";
const MANIFEST_FILE: &str = "manifest.json";
const GENERATION_FILE: &str = ".generation";
const CACHE_LOCK_FILE: &str = ".cache.lock";
const LOCK_FOLDER: &str = ".locks";
const LEASE_FOLDER: &str = ".leases";
const MANIFEST_VERSION: u32 = 2;
const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(5 * 60);
const STAGING_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

static LEASE_TOKEN: AtomicU64 = AtomicU64::new(0);
static LEASE_PROCESS_NONCE: OnceLock<String> = OnceLock::new();

struct StagedPathCleanup(PathBuf);

impl Drop for StagedPathCleanup {
    fn drop(&mut self) {
        match fs::remove_file(&self.0) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "failed to clean cache staging file {}: {error}",
                self.0.display()
            ),
        }
    }
}

pub trait MediaCache: Send + Sync {
    fn generation(&self) -> Result<u64, MediaError>;
    fn lookup_or_generation(&self, request: &CacheRequest) -> Result<CacheLookup, MediaError>;
    fn lookup(&self, request: &CacheRequest) -> Result<Option<CacheHit>, MediaError> {
        Ok(match self.lookup_or_generation(request)? {
            CacheLookup::Hit(hit) => Some(*hit),
            CacheLookup::Generate { .. } => None,
        })
    }
    fn planned_location(&self, artifact: &PendingArtifact) -> Result<Option<PathBuf>, MediaError>;
    fn publish(&self, artifact: PendingArtifact) -> Result<CachePublication, MediaError>;

    fn publish_staged(
        &self,
        artifact: PendingStagedArtifact,
    ) -> Result<CachePublication, MediaError> {
        let _cleanup = StagedPathCleanup(artifact.staged_path.clone());
        let bytes = Arc::from(fs::read(&artifact.staged_path)?);
        let pending = PendingArtifact {
            source_revision: artifact.source_revision,
            variant: artifact.variant,
            actual_dimensions: artifact.actual_dimensions,
            native_detail: artifact.native_detail,
            media_type: artifact.media_type,
            extension: artifact.extension,
            bytes,
            cache_generation: artifact.cache_generation,
        };
        self.publish(pending)
    }

    fn lease_artifact(
        &self,
        _artifact: &MediaArtifact,
    ) -> Result<Option<ArtifactLease>, MediaError> {
        Ok(None)
    }

    fn clear(&self) -> Result<(), MediaError>;
}

#[derive(Clone)]
pub struct PendingArtifact {
    pub source_revision: SourceRevision,
    pub variant: VariantIdentity,
    pub actual_dimensions: DisplayDimensions,
    pub native_detail: bool,
    pub media_type: String,
    pub extension: String,
    pub bytes: Arc<[u8]>,
    pub cache_generation: u64,
}

pub struct PendingStagedArtifact {
    pub source_revision: SourceRevision,
    pub variant: VariantIdentity,
    pub actual_dimensions: DisplayDimensions,
    pub native_detail: bool,
    pub media_type: String,
    pub extension: String,
    pub staged_path: PathBuf,
    pub cache_generation: u64,
}

pub struct CacheHit {
    pub artifact: MediaArtifact,
    pub satisfaction: Satisfaction,
    pub lease: ArtifactLease,
}

pub enum CacheLookup {
    Hit(Box<CacheHit>),
    Generate { cache_generation: u64 },
}

pub struct CachePublication {
    pub artifact: MediaArtifact,
    pub lease: Option<ArtifactLease>,
}

pub struct ArtifactLease {
    state: Arc<LeaseState>,
    token: u64,
}

impl ArtifactLease {
    pub fn renew(&self) -> Result<(), MediaError> {
        self.state.renew(self.token)
    }
}

impl Drop for ArtifactLease {
    fn drop(&mut self) {
        self.state.release(self.token);
    }
}

#[derive(Default)]
struct LeaseState {
    inner: Mutex<LeaseInner>,
}

#[derive(Default)]
struct LeaseInner {
    leases: HashMap<u64, LeaseRecord>,
}

struct LeaseRecord {
    path: PathBuf,
    expires: Instant,
    ttl: Duration,
    marker: Option<PathBuf>,
}

impl LeaseState {
    fn acquire(
        self: &Arc<Self>,
        path: PathBuf,
        ttl: Duration,
        marker_dir: Option<&Path>,
    ) -> Result<ArtifactLease, MediaError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let token = LEASE_TOKEN.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let marker = marker_dir
            .map(|directory| create_lease_marker(directory, &path, token))
            .transpose()?;
        inner.leases.insert(
            token,
            LeaseRecord {
                path,
                expires: Instant::now() + ttl,
                ttl,
                marker,
            },
        );
        Ok(ArtifactLease {
            state: Arc::clone(self),
            token,
        })
    }

    fn renew(&self, token: u64) -> Result<(), MediaError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(lease) = inner.leases.get_mut(&token) else {
            return Err(MediaError::CacheArtifact(
                "artifact lease has expired".into(),
            ));
        };
        lease.expires = Instant::now() + lease.ttl;
        if let Some(marker) = &lease.marker {
            let mut marker = OpenOptions::new().write(true).truncate(true).open(marker)?;
            writeln!(marker, "{}", unix_millis())?;
        }
        Ok(())
    }

    fn release(&self, token: u64) {
        let lease = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .leases
            .remove(&token);
        if let Some(marker) = lease.and_then(|lease| lease.marker) {
            let _ = fs::remove_file(marker);
        }
    }

    fn protected_paths(&self) -> Vec<PathBuf> {
        let now = Instant::now();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.leases.retain(|_, lease| lease.expires > now);
        inner
            .leases
            .values()
            .map(|lease| lease.path.clone())
            .collect()
    }
}

#[derive(Clone)]
pub struct DiskMediaCache {
    inner: Arc<DiskCacheInner>,
}

struct DiskCacheInner {
    root: PathBuf,
    max_manifests: usize,
    operation: Mutex<()>,
    index: Mutex<ManifestIndex>,
    leases: Arc<LeaseState>,
    lease_ttl: Duration,
}

#[derive(Default)]
struct ManifestIndex {
    entries: HashMap<String, CachedManifest>,
    lru: VecDeque<String>,
}

#[derive(Clone)]
struct CachedManifest {
    modified: Option<SystemTime>,
    manifest: Manifest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: u32,
    cache_generation: u64,
    source_revision: SourceRevision,
    artifacts: Vec<ManifestArtifact>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestArtifact {
    artifact_id: String,
    variant: VariantIdentity,
    actual_dimensions: DisplayDimensions,
    native_detail: bool,
    byte_size: u64,
    media_type: String,
    file_name: String,
    last_used_unix_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheUsage {
    pub size_bytes: u64,
    pub artifact_count: usize,
}

impl DiskMediaCache {
    pub fn new(cache_parent: impl AsRef<Path>, max_manifests: usize) -> Result<Self, MediaError> {
        Self::with_lease_ttl(cache_parent, max_manifests, DEFAULT_LEASE_TTL)
    }

    fn with_lease_ttl(
        cache_parent: impl AsRef<Path>,
        max_manifests: usize,
        lease_ttl: Duration,
    ) -> Result<Self, MediaError> {
        fs::create_dir_all(cache_parent.as_ref())?;
        // Lease markers hash artifact paths. Publishers and maintenance must
        // use the same spelling, including Windows' canonical path prefix.
        let root = cache_parent.as_ref().canonicalize()?.join(CACHE_FOLDER);
        ensure_owned_directory(&root)?;
        ensure_owned_directory(&root.join(LOCK_FOLDER))?;
        ensure_owned_directory(&root.join(LEASE_FOLDER))?;
        ensure_generation(&root)?;
        ensure_regular_control_file(&root.join(GENERATION_FILE))?;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(CACHE_LOCK_FILE))?;
        ensure_regular_control_file(&root.join(CACHE_LOCK_FILE))?;
        cleanup_staging(&root, false, &[])?;
        Ok(Self {
            inner: Arc::new(DiskCacheInner {
                root,
                max_manifests: max_manifests.max(1),
                operation: Mutex::new(()),
                index: Mutex::new(ManifestIndex::default()),
                leases: Arc::new(LeaseState::default()),
                lease_ttl,
            }),
        })
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    pub fn lease_path(&self, path: &Path) -> Result<ArtifactLease, MediaError> {
        if !path.starts_with(&self.inner.root) || !fs::symlink_metadata(path)?.file_type().is_file()
        {
            return Err(MediaError::CacheArtifact(
                "only regular managed artifacts can hold disk leases".into(),
            ));
        }
        self.inner.leases.acquire(
            path.to_owned(),
            self.inner.lease_ttl,
            Some(&self.inner.root.join(LEASE_FOLDER)),
        )
    }

    /// Validates a restart-restored managed path against its current manifest
    /// entry before leasing it. A bare file in the cache tree is not a cache
    /// hit: it must still have complete bytes and the recorded dimensions.
    pub fn validate_and_lease_path(
        &self,
        path: &Path,
        expected_source: &SourceRevision,
    ) -> Result<Option<ArtifactLease>, MediaError> {
        let Some(source_dir) = path.parent() else {
            return Ok(None);
        };
        let Some(revision_id) = source_dir.file_name().and_then(std::ffi::OsStr::to_str) else {
            return Ok(None);
        };
        let Some(prefix) = source_dir
            .parent()
            .and_then(Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
        else {
            return Ok(None);
        };
        if source_dir.parent().and_then(Path::parent) != Some(self.inner.root.as_path())
            || !is_lower_hex(revision_id, 64)
            || prefix != &revision_id[..2]
        {
            return Ok(None);
        }

        let _operation = self
            .inner
            .operation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cache_lock = self.open_cache_lock()?;
        FileExt::lock_shared(&cache_lock)?;
        let generation = self.current_generation_unlocked()?;
        let source_lock = self.open_revision_lock(revision_id)?;
        FileExt::lock_exclusive(&source_lock)?;

        let validation = (|| {
            let manifest_path = source_dir.join(MANIFEST_FILE);
            reject_symlink(&manifest_path)?;
            let bytes = match fs::read(&manifest_path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            let mut manifest: Manifest = match serde_json::from_slice(&bytes) {
                Ok(manifest) => manifest,
                Err(_) => {
                    fs::remove_file(&manifest_path)?;
                    return Ok(None);
                }
            };
            if manifest.version != MANIFEST_VERSION
                || manifest.cache_generation != generation
                || manifest.source_revision.revision_id != revision_id
                || manifest.source_revision != *expected_source
                || validate_source_revision(&manifest.source_revision).is_err()
                || SourceRevision::observe(&manifest.source_revision.canonical_path)?
                    != manifest.source_revision
            {
                return Ok(None);
            }
            let original_len = manifest.artifacts.len();
            retain_valid_artifacts(source_dir, &mut manifest.artifacts)?;
            if manifest.artifacts.len() != original_len {
                self.write_manifest(source_dir, &manifest)?;
            }
            if !manifest
                .artifacts
                .iter()
                .any(|artifact| source_dir.join(&artifact.file_name) == path)
            {
                return Ok(None);
            }
            self.inner
                .leases
                .acquire(
                    path.to_owned(),
                    self.inner.lease_ttl,
                    Some(&self.inner.root.join(LEASE_FOLDER)),
                )
                .map(Some)
        })();
        FileExt::unlock(&source_lock)?;
        FileExt::unlock(&cache_lock)?;
        validation
    }

    pub fn lease_artifact(&self, artifact: &MediaArtifact) -> Result<ArtifactLease, MediaError> {
        let ArtifactLocation::Managed(path) = &artifact.location else {
            return Err(MediaError::CacheArtifact(
                "only managed artifacts can hold disk leases".into(),
            ));
        };
        self.lease_path(path)
    }

    pub fn publish_staged(
        &self,
        pending: PendingStagedArtifact,
    ) -> Result<CachePublication, MediaError> {
        self.commit_staged(pending)
    }

    fn publish_staged_copy(
        &self,
        mut pending: PendingStagedArtifact,
    ) -> Result<CachePublication, MediaError> {
        let (copy_path, _copy_lease) = {
            // Clear/prune use the same operation mutex. Hold it from creation
            // until a lease protects the complete worker-owned copy.
            let _operation = self
                .inner
                .operation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let staging = self.inner.root.join(".backend-tmp");
            ensure_owned_directory(&staging)?;
            let copy = tempfile::Builder::new()
                .suffix(".jpg")
                .tempfile_in(&staging)?
                .into_temp_path();
            fs::copy(&pending.staged_path, &copy)?;
            let copy_path = copy.keep().map_err(|error| MediaError::Io(error.error))?;
            let lease = self.inner.leases.acquire(
                copy_path.clone(),
                self.inner.lease_ttl,
                Some(&self.inner.root.join(LEASE_FOLDER)),
            )?;
            (copy_path, lease)
        };
        let _copy_cleanup = StagedPathCleanup(copy_path.clone());
        pending.staged_path = copy_path;
        self.commit_staged(pending)
    }

    fn commit_staged(
        &self,
        pending: PendingStagedArtifact,
    ) -> Result<CachePublication, MediaError> {
        let _staged_cleanup = StagedPathCleanup(pending.staged_path.clone());
        validate_source_revision(&pending.source_revision)?;
        validate_extension(&pending.extension)?;
        let staging = self.inner.root.join(".backend-tmp");
        ensure_owned_directory(&staging)?;
        if pending.staged_path.parent() != Some(staging.as_path())
            || !fs::symlink_metadata(&pending.staged_path)?
                .file_type()
                .is_file()
        {
            return Err(MediaError::CacheArtifact(
                "native output is outside the owned staging directory".into(),
            ));
        }
        let _operation = self
            .inner
            .operation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cache_lock = self.open_cache_lock()?;
        FileExt::lock_shared(&cache_lock)?;
        let generation = self.current_generation_unlocked()?;
        if generation != pending.cache_generation {
            FileExt::unlock(&cache_lock)?;
            return Err(MediaError::StaleCacheGeneration);
        }
        if SourceRevision::observe(&pending.source_revision.canonical_path)?
            != pending.source_revision
        {
            FileExt::unlock(&cache_lock)?;
            return Err(MediaError::StaleSourceRevision);
        }
        let source_lock = self.open_source_lock(&pending.source_revision)?;
        FileExt::lock_exclusive(&source_lock)?;
        let source_dir = self.ensure_source_dir(&pending.source_revision)?;
        let artifact_id = artifact_id_staged(&pending)?;
        let file_name = format!("{artifact_id}.{}", pending.extension);
        let destination = source_dir.join(&file_name);
        let created = if existing_regular_file(&destination)? {
            fs::remove_file(&pending.staged_path)?;
            false
        } else {
            fs::rename(&pending.staged_path, &destination)?;
            sync_directory(&source_dir)?;
            true
        };
        let dimensions = image::image_dimensions(&destination)?;
        if !dimensions_match(
            dimensions,
            pending.actual_dimensions,
            pending.variant.presentation.orientation,
        ) {
            if created {
                let _ = fs::remove_file(&destination);
            }
            return Err(MediaError::CacheArtifact(
                "published dimensions do not match native output".into(),
            ));
        }
        if let Err(error) = validate_jpeg_completion(&destination, &pending.media_type) {
            if created {
                let _ = fs::remove_file(&destination);
            }
            return Err(error);
        }
        let byte_size = fs::metadata(&destination)?.len();
        if SourceRevision::observe(&pending.source_revision.canonical_path)?
            != pending.source_revision
        {
            if created {
                let _ = fs::remove_file(&destination);
            }
            return Err(MediaError::StaleSourceRevision);
        }
        let mut manifest = self.load_manifest(&pending.source_revision, generation, false)?;
        manifest
            .artifacts
            .retain(|artifact| artifact.artifact_id != artifact_id);
        manifest.artifacts.push(ManifestArtifact {
            artifact_id: artifact_id.clone(),
            variant: pending.variant.clone(),
            actual_dimensions: pending.actual_dimensions,
            native_detail: pending.native_detail,
            byte_size,
            media_type: pending.media_type.clone(),
            file_name,
            last_used_unix_ms: unix_millis(),
        });
        self.write_manifest(&source_dir, &manifest)?;
        self.index_put(
            pending.source_revision.revision_id.clone(),
            fs::metadata(source_dir.join(MANIFEST_FILE))
                .and_then(|metadata| metadata.modified())
                .ok(),
            manifest,
        );
        let lease = self.inner.leases.acquire(
            destination.clone(),
            self.inner.lease_ttl,
            Some(&self.inner.root.join(LEASE_FOLDER)),
        )?;
        FileExt::unlock(&source_lock)?;
        FileExt::unlock(&cache_lock)?;
        Ok(CachePublication {
            artifact: MediaArtifact {
                artifact_id,
                source_revision: pending.source_revision,
                variant: pending.variant,
                actual_dimensions: pending.actual_dimensions,
                native_detail: pending.native_detail,
                byte_size,
                media_type: pending.media_type,
                location: ArtifactLocation::Managed(destination),
            },
            lease: Some(lease),
        })
    }

    fn protected_paths(&self) -> Result<Vec<PathBuf>, MediaError> {
        let mut protected = self.inner.leases.protected_paths();
        let mut protected_ids = Vec::new();
        let now = SystemTime::now();
        for marker in fs::read_dir(self.inner.root.join(LEASE_FOLDER))? {
            let marker = marker?;
            if !marker.file_type()?.is_file() {
                continue;
            }
            let metadata = marker.metadata()?;
            let expired = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > self.inner.lease_ttl);
            if expired {
                let _ = fs::remove_file(marker.path());
                continue;
            }
            let name = marker.file_name().to_string_lossy().into_owned();
            if let Some(lease_id) = name.get(..64) {
                protected_ids.push(lease_id.to_owned());
            }
        }
        for artifact in collect_artifacts(&self.inner.root)? {
            if protected_ids
                .iter()
                .any(|protected| protected == &lease_marker_id(&artifact.path))
            {
                protected.push(artifact.path);
            }
        }
        Ok(protected)
    }

    pub fn usage(&self) -> Result<CacheUsage, MediaError> {
        let _operation = self
            .inner
            .operation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cache_lock = self.open_cache_lock()?;
        FileExt::lock_shared(&cache_lock)?;
        let usage = collect_artifacts(&self.inner.root)?.into_iter().fold(
            CacheUsage {
                size_bytes: 0,
                artifact_count: 0,
            },
            |mut usage, artifact| {
                usage.size_bytes = usage.size_bytes.saturating_add(artifact.size_bytes);
                usage.artifact_count += 1;
                usage
            },
        );
        FileExt::unlock(&cache_lock)?;
        Ok(usage)
    }

    pub fn prune(&self, max_size_bytes: u64) -> Result<CacheUsage, MediaError> {
        self.prune_with_protected(max_size_bytes, None)
    }

    pub fn prune_with_protected(
        &self,
        max_size_bytes: u64,
        protected_path: Option<&Path>,
    ) -> Result<CacheUsage, MediaError> {
        let _operation = self
            .inner
            .operation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cache_lock = self.open_cache_lock()?;
        FileExt::lock_exclusive(&cache_lock)?;
        let mut artifacts = collect_artifacts(&self.inner.root)?;
        let mut total = artifacts
            .iter()
            .map(|artifact| artifact.size_bytes)
            .sum::<u64>();
        // A completed thumbnail can request maintenance while the next one
        // is looking up its cache. Under budget, do not hold the global
        // exclusive lock to scan leases or rewrite/fsync every manifest.
        if total <= max_size_bytes {
            FileExt::unlock(&cache_lock)?;
            return Ok(CacheUsage {
                size_bytes: total,
                artifact_count: artifacts.len(),
            });
        }
        let mut protected = self.protected_paths()?;
        if let Some(path) = protected_path {
            protected.push(path.to_owned());
        }
        artifacts.sort_by_key(|artifact| artifact.modified);
        for artifact in &artifacts {
            if total <= max_size_bytes {
                break;
            }
            if protected.contains(&artifact.path) {
                continue;
            }
            match fs::remove_file(&artifact.path) {
                Ok(()) => total = total.saturating_sub(artifact.size_bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        repair_all_manifests(&self.inner.root, self.current_generation_unlocked()?)?;
        self.inner
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .clear();
        FileExt::unlock(&cache_lock)?;
        let remaining = collect_artifacts(&self.inner.root)?;
        Ok(CacheUsage {
            size_bytes: remaining.iter().map(|entry| entry.size_bytes).sum(),
            artifact_count: remaining.len(),
        })
    }

    fn source_dir(&self, source: &SourceRevision) -> PathBuf {
        self.inner
            .root
            .join(&source.revision_id[..2])
            .join(&source.revision_id)
    }

    fn ensure_source_dir(&self, source: &SourceRevision) -> Result<PathBuf, MediaError> {
        let prefix = self.inner.root.join(&source.revision_id[..2]);
        ensure_owned_directory(&prefix)?;
        let source_dir = prefix.join(&source.revision_id);
        ensure_owned_directory(&source_dir)?;
        Ok(source_dir)
    }

    fn validate_existing_source_dir(
        &self,
        source: &SourceRevision,
    ) -> Result<Option<PathBuf>, MediaError> {
        let prefix = self.inner.root.join(&source.revision_id[..2]);
        if !validate_optional_owned_directory(&prefix)? {
            return Ok(None);
        }
        let source_dir = prefix.join(&source.revision_id);
        if !validate_optional_owned_directory(&source_dir)? {
            return Ok(None);
        }
        Ok(Some(source_dir))
    }

    fn open_cache_lock(&self) -> Result<File, MediaError> {
        let path = self.inner.root.join(CACHE_LOCK_FILE);
        ensure_regular_control_file(&path)?;
        Ok(OpenOptions::new().read(true).write(true).open(path)?)
    }

    fn open_source_lock(&self, source: &SourceRevision) -> Result<File, MediaError> {
        self.open_revision_lock(&source.revision_id)
    }

    fn open_revision_lock(&self, revision_id: &str) -> Result<File, MediaError> {
        let directory = self.inner.root.join(LOCK_FOLDER);
        ensure_owned_directory(&directory)?;
        let path = directory.join(format!("{revision_id}.lock"));
        reject_symlink(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        ensure_regular_control_file(&path)?;
        Ok(file)
    }

    fn current_generation_unlocked(&self) -> Result<u64, MediaError> {
        read_generation(&self.inner.root)
    }

    fn load_manifest(
        &self,
        source: &SourceRevision,
        generation: u64,
        allow_cached: bool,
    ) -> Result<Manifest, MediaError> {
        let Some(source_dir) = self.validate_existing_source_dir(source)? else {
            return Ok(empty_manifest(source.clone(), generation));
        };
        let manifest_path = source_dir.join(MANIFEST_FILE);
        reject_symlink(&manifest_path)?;
        let modified = match fs::metadata(&manifest_path) {
            Ok(metadata) => metadata.modified().ok(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if allow_cached
            && let Some(cached) = self.index_get(&source.revision_id)
            && cached.modified == modified
            && cached.manifest.cache_generation == generation
        {
            return Ok(cached.manifest);
        }
        let manifest = match fs::read(&manifest_path) {
            Ok(bytes) => match serde_json::from_slice::<Manifest>(&bytes) {
                Ok(manifest)
                    if manifest.version == MANIFEST_VERSION
                        && manifest.cache_generation == generation
                        && manifest.source_revision == *source =>
                {
                    manifest
                }
                Ok(_) => empty_manifest(source.clone(), generation),
                Err(_) => {
                    match fs::remove_file(&manifest_path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                    empty_manifest(source.clone(), generation)
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                empty_manifest(source.clone(), generation)
            }
            Err(error) => return Err(error.into()),
        };
        self.index_put(source.revision_id.clone(), modified, manifest.clone());
        Ok(manifest)
    }

    fn index_get(&self, key: &str) -> Option<CachedManifest> {
        let mut index = self
            .inner
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = index.entries.get(key).cloned();
        if value.is_some() {
            index.lru.retain(|candidate| candidate != key);
            index.lru.push_back(key.to_owned());
        }
        value
    }

    fn index_put(&self, key: String, modified: Option<SystemTime>, manifest: Manifest) {
        let mut index = self
            .inner
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        index.lru.retain(|candidate| candidate != &key);
        index.lru.push_back(key.clone());
        index
            .entries
            .insert(key, CachedManifest { modified, manifest });
        while index.entries.len() > self.inner.max_manifests {
            if let Some(expired) = index.lru.pop_front() {
                index.entries.remove(&expired);
            }
        }
    }

    fn write_manifest(&self, source_dir: &Path, manifest: &Manifest) -> Result<(), MediaError> {
        ensure_owned_directory(source_dir)?;
        let mut temporary = NamedTempFile::new_in(source_dir)?;
        serde_json::to_writer(&mut temporary, manifest)
            .map_err(|error| MediaError::CacheManifest(error.to_string()))?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(source_dir.join(MANIFEST_FILE))
            .map_err(|error| MediaError::Io(error.error))?;
        sync_directory(source_dir)?;
        Ok(())
    }
}

impl MediaCache for DiskMediaCache {
    fn generation(&self) -> Result<u64, MediaError> {
        // The generation file is protected by the cross-process cache lock.
        // Do not serialize this read behind a publisher's in-process mutex:
        // active UI lookups must not wait for unrelated artifact fsync/SMB I/O.
        let cache_lock = self.open_cache_lock()?;
        FileExt::lock_shared(&cache_lock)?;
        let generation = self.current_generation_unlocked()?;
        FileExt::unlock(&cache_lock)?;
        Ok(generation)
    }

    fn lookup_or_generation(&self, request: &CacheRequest) -> Result<CacheLookup, MediaError> {
        validate_source_revision(&request.source_revision)?;
        let _operation = self
            .inner
            .operation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cache_lock = self.open_cache_lock()?;
        FileExt::lock_shared(&cache_lock)?;
        let generation = self.current_generation_unlocked()?;
        let source_lock = self.open_source_lock(&request.source_revision)?;
        FileExt::lock_exclusive(&source_lock)?;
        let source_dir = self.source_dir(&request.source_revision);
        // Always read the small per-source manifest while holding its
        // cross-process lock. Filesystem timestamps are not a coherence token.
        let mut manifest = self.load_manifest(&request.source_revision, generation, false)?;
        let original_len = manifest.artifacts.len();
        retain_valid_artifacts(&source_dir, &mut manifest.artifacts)?;
        if manifest.artifacts.len() != original_len {
            self.write_manifest(&source_dir, &manifest)?;
        }
        let best = manifest
            .artifacts
            .iter()
            .filter_map(|stored| {
                let artifact = stored.to_artifact(&manifest.source_revision, &source_dir);
                satisfies(&artifact, request).map(|satisfaction| (artifact, satisfaction))
            })
            .min_by_key(|(artifact, satisfaction)| candidate_rank(artifact, *satisfaction));
        self.index_put(
            request.source_revision.revision_id.clone(),
            fs::metadata(source_dir.join(MANIFEST_FILE))
                .and_then(|metadata| metadata.modified())
                .ok(),
            manifest,
        );
        let lookup = match best {
            Some((artifact, satisfaction)) => {
                let ArtifactLocation::Managed(path) = &artifact.location else {
                    unreachable!("v2 manifests contain only managed artifacts")
                };
                let lease = self.inner.leases.acquire(
                    path.clone(),
                    self.inner.lease_ttl,
                    Some(&self.inner.root.join(LEASE_FOLDER)),
                )?;
                CacheLookup::Hit(Box::new(CacheHit {
                    artifact,
                    satisfaction,
                    lease,
                }))
            }
            None => CacheLookup::Generate {
                cache_generation: generation,
            },
        };
        FileExt::unlock(&source_lock)?;
        FileExt::unlock(&cache_lock)?;
        Ok(lookup)
    }

    fn planned_location(&self, pending: &PendingArtifact) -> Result<Option<PathBuf>, MediaError> {
        validate_source_revision(&pending.source_revision)?;
        validate_extension(&pending.extension)?;
        Ok(Some(self.source_dir(&pending.source_revision).join(
            format!("{}.{}", artifact_id(pending), pending.extension),
        )))
    }

    fn publish(&self, pending: PendingArtifact) -> Result<CachePublication, MediaError> {
        validate_source_revision(&pending.source_revision)?;
        validate_extension(&pending.extension)?;
        let _operation = self
            .inner
            .operation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cache_lock = self.open_cache_lock()?;
        FileExt::lock_shared(&cache_lock)?;
        let generation = self.current_generation_unlocked()?;
        if generation != pending.cache_generation {
            FileExt::unlock(&cache_lock)?;
            return Err(MediaError::StaleCacheGeneration);
        }
        let observed = SourceRevision::observe(&pending.source_revision.canonical_path)?;
        if observed != pending.source_revision {
            FileExt::unlock(&cache_lock)?;
            return Err(MediaError::StaleSourceRevision);
        }
        let source_lock = self.open_source_lock(&pending.source_revision)?;
        FileExt::lock_exclusive(&source_lock)?;
        let source_dir = self.ensure_source_dir(&pending.source_revision)?;
        ensure_owned_directory(&source_dir.join(".tmp"))?;
        let artifact_id = artifact_id(&pending);
        let file_name = format!("{artifact_id}.{}", pending.extension);
        let destination = source_dir.join(&file_name);
        let created = if existing_regular_file(&destination)? {
            false
        } else {
            let mut temporary = NamedTempFile::new_in(source_dir.join(".tmp"))?;
            temporary.write_all(&pending.bytes)?;
            temporary.as_file().sync_all()?;
            match temporary.persist_noclobber(&destination) {
                Ok(_) => sync_directory(&source_dir)?,
                Err(error) if existing_regular_file(&destination)? => drop(error),
                Err(error) => return Err(MediaError::Io(error.error)),
            }
            true
        };
        let dimensions = image::image_dimensions(&destination)?;
        if !dimensions_match(
            dimensions,
            pending.actual_dimensions,
            pending.variant.presentation.orientation,
        ) {
            if created {
                let _ = fs::remove_file(&destination);
            }
            return Err(MediaError::CacheArtifact(
                "published dimensions do not match encoded payload".into(),
            ));
        }
        if let Err(error) = validate_jpeg_completion(&destination, &pending.media_type) {
            if created {
                let _ = fs::remove_file(&destination);
            }
            return Err(error);
        }
        let byte_size = fs::metadata(&destination)?.len();
        let observed = SourceRevision::observe(&pending.source_revision.canonical_path)?;
        if observed != pending.source_revision {
            if created {
                let _ = fs::remove_file(&destination);
            }
            FileExt::unlock(&source_lock)?;
            FileExt::unlock(&cache_lock)?;
            return Err(MediaError::StaleSourceRevision);
        }
        // Always merge publication with a fresh on-disk snapshot. Metadata
        // timestamps can be coarse on network filesystems, so the bounded
        // lookup index is not sufficient for a cross-process write decision.
        let mut manifest = self.load_manifest(&pending.source_revision, generation, false)?;
        manifest
            .artifacts
            .retain(|artifact| artifact.artifact_id != artifact_id);
        manifest.artifacts.push(ManifestArtifact {
            artifact_id: artifact_id.clone(),
            variant: pending.variant.clone(),
            actual_dimensions: pending.actual_dimensions,
            native_detail: pending.native_detail,
            byte_size,
            media_type: pending.media_type.clone(),
            file_name,
            last_used_unix_ms: unix_millis(),
        });
        self.write_manifest(&source_dir, &manifest)?;
        self.index_put(
            pending.source_revision.revision_id.clone(),
            fs::metadata(source_dir.join(MANIFEST_FILE))
                .and_then(|metadata| metadata.modified())
                .ok(),
            manifest,
        );
        let lease = self.inner.leases.acquire(
            destination.clone(),
            self.inner.lease_ttl,
            Some(&self.inner.root.join(LEASE_FOLDER)),
        )?;
        FileExt::unlock(&source_lock)?;
        FileExt::unlock(&cache_lock)?;
        Ok(CachePublication {
            artifact: MediaArtifact {
                artifact_id,
                source_revision: pending.source_revision,
                variant: pending.variant,
                actual_dimensions: pending.actual_dimensions,
                native_detail: pending.native_detail,
                byte_size,
                media_type: pending.media_type,
                location: ArtifactLocation::Managed(destination),
            },
            lease: Some(lease),
        })
    }

    fn publish_staged(
        &self,
        artifact: PendingStagedArtifact,
    ) -> Result<CachePublication, MediaError> {
        // Keep the registry-owned native output stable while cache publication
        // proceeds. The cache takes ownership of a file-system copy and may
        // atomically rename that copy without invalidating an in-flight read.
        self.publish_staged_copy(artifact)
    }

    fn lease_artifact(
        &self,
        artifact: &MediaArtifact,
    ) -> Result<Option<ArtifactLease>, MediaError> {
        DiskMediaCache::lease_artifact(self, artifact).map(Some)
    }

    fn clear(&self) -> Result<(), MediaError> {
        let _operation = self
            .inner
            .operation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cache_lock = self.open_cache_lock()?;
        FileExt::lock_exclusive(&cache_lock)?;
        let generation = self.current_generation_unlocked()?.wrapping_add(1);
        write_generation(&self.inner.root, generation)?;
        let protected = self.protected_paths()?;
        cleanup_staging(&self.inner.root, true, &protected)?;
        for entry in fs::read_dir(&self.inner.root)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !file_type.is_dir() || !is_lower_hex(&name, 2) {
                continue;
            }
            remove_owned_prefix(&entry.path(), &name, &protected)?;
        }
        *self
            .inner
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ManifestIndex::default();
        FileExt::unlock(&cache_lock)?;
        Ok(())
    }
}

#[derive(Default)]
pub struct MemoryMediaCache {
    state: Mutex<MemoryState>,
    leases: Arc<LeaseState>,
}

#[derive(Default)]
struct MemoryState {
    generation: u64,
    artifacts: Vec<MediaArtifact>,
}

impl MediaCache for MemoryMediaCache {
    fn generation(&self) -> Result<u64, MediaError> {
        Ok(self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation)
    }

    fn lookup_or_generation(&self, request: &CacheRequest) -> Result<CacheLookup, MediaError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let best = state
            .artifacts
            .iter()
            .filter_map(|artifact| {
                satisfies(artifact, request).map(|satisfaction| (artifact.clone(), satisfaction))
            })
            .min_by_key(|(artifact, satisfaction)| candidate_rank(artifact, *satisfaction));
        Ok(match best {
            Some((artifact, satisfaction)) => CacheLookup::Hit(Box::new(CacheHit {
                lease: self
                    .leases
                    .acquire(
                        PathBuf::from(&artifact.artifact_id),
                        DEFAULT_LEASE_TTL,
                        None,
                    )
                    .expect("memory leases do not perform I/O"),
                artifact,
                satisfaction,
            })),
            None => CacheLookup::Generate {
                cache_generation: state.generation,
            },
        })
    }

    fn planned_location(&self, _pending: &PendingArtifact) -> Result<Option<PathBuf>, MediaError> {
        Ok(None)
    }

    fn publish(&self, pending: PendingArtifact) -> Result<CachePublication, MediaError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.generation != pending.cache_generation {
            return Err(MediaError::StaleCacheGeneration);
        }
        let artifact = MediaArtifact {
            artifact_id: artifact_id(&pending),
            source_revision: pending.source_revision,
            variant: pending.variant,
            actual_dimensions: pending.actual_dimensions,
            native_detail: pending.native_detail,
            byte_size: pending.bytes.len() as u64,
            media_type: pending.media_type,
            location: ArtifactLocation::Managed(PathBuf::from("memory")),
        };
        state
            .artifacts
            .retain(|candidate| candidate.artifact_id != artifact.artifact_id);
        state.artifacts.push(artifact.clone());
        let lease = self
            .leases
            .acquire(
                PathBuf::from(&artifact.artifact_id),
                DEFAULT_LEASE_TTL,
                None,
            )
            .expect("memory leases do not perform I/O");
        Ok(CachePublication {
            artifact,
            lease: Some(lease),
        })
    }

    fn clear(&self) -> Result<(), MediaError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.generation = state.generation.wrapping_add(1);
        state.artifacts.clear();
        Ok(())
    }
}

impl ManifestArtifact {
    fn to_artifact(&self, source: &SourceRevision, source_dir: &Path) -> MediaArtifact {
        MediaArtifact {
            artifact_id: self.artifact_id.clone(),
            source_revision: source.clone(),
            variant: self.variant.clone(),
            actual_dimensions: self.actual_dimensions,
            native_detail: self.native_detail,
            byte_size: self.byte_size,
            media_type: self.media_type.clone(),
            location: ArtifactLocation::Managed(source_dir.join(&self.file_name)),
        }
    }
}

fn empty_manifest(source_revision: SourceRevision, cache_generation: u64) -> Manifest {
    Manifest {
        version: MANIFEST_VERSION,
        cache_generation,
        source_revision,
        artifacts: Vec::new(),
    }
}

fn ensure_owned_directory(path: &Path) -> Result<(), MediaError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => return Ok(()),
        Ok(_) => {
            return Err(MediaError::CacheArtifact(format!(
                "owned cache path is not a directory: {}",
                path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::create_dir(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(MediaError::CacheArtifact(format!(
            "owned cache path became unsafe: {}",
            path.display()
        )));
    }
    Ok(())
}

fn existing_regular_file(path: &Path) -> Result<bool, MediaError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(MediaError::CacheArtifact(format!(
            "owned artifact path is not a regular file: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn validate_optional_owned_directory(path: &Path) -> Result<bool, MediaError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => Err(MediaError::CacheArtifact(format!(
            "owned cache path is not a directory: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn reject_symlink(path: &Path) -> Result<(), MediaError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(MediaError::CacheArtifact(
            format!("owned cache path is a symlink: {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn ensure_regular_control_file(path: &Path) -> Result<(), MediaError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(MediaError::CacheArtifact(format!(
            "cache control path is not a regular file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn cleanup_staging(root: &Path, remove_all: bool, protected: &[PathBuf]) -> Result<(), MediaError> {
    remove_regular_files(&root.join(".backend-tmp"), remove_all, protected)?;
    for prefix in fs::read_dir(root)? {
        let prefix = prefix?;
        let prefix_name = prefix.file_name().to_string_lossy().into_owned();
        if !prefix.file_type()?.is_dir() || !is_lower_hex(&prefix_name, 2) {
            continue;
        }
        for source in fs::read_dir(prefix.path())? {
            let source = source?;
            let source_name = source.file_name().to_string_lossy().into_owned();
            if source.file_type()?.is_dir()
                && is_lower_hex(&source_name, 64)
                && source_name.starts_with(&prefix_name)
            {
                remove_regular_files(&source.path().join(".tmp"), remove_all, protected)?;
            }
        }
    }
    Ok(())
}

fn remove_regular_files(
    directory: &Path,
    remove_all: bool,
    protected: &[PathBuf],
) -> Result<(), MediaError> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_dir() {
        return Err(MediaError::CacheArtifact(format!(
            "owned staging path is not a directory: {}",
            directory.display()
        )));
    }
    let now = SystemTime::now();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let stale = remove_all
            || entry
                .metadata()?
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= STAGING_MAX_AGE);
        if stale && !protected.contains(&entry.path()) {
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

fn create_lease_marker(
    directory: &Path,
    artifact_path: &Path,
    token: u64,
) -> Result<PathBuf, MediaError> {
    let artifact_id = lease_marker_id(artifact_path);
    let nonce = LEASE_PROCESS_NONCE.get_or_init(|| {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{}-{now:x}", std::process::id())
    });
    for collision in 0_u8..16 {
        let marker = directory.join(format!("{artifact_id}-{nonce}-{token}-{collision}"));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&marker)
        {
            Ok(mut file) => {
                writeln!(file, "{}", unix_millis())?;
                return Ok(marker);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(MediaError::CacheArtifact(
        "could not allocate a unique artifact lease marker".into(),
    ))
}

fn lease_marker_id(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_os_str().as_encoded_bytes());
    format!("{:x}", hasher.finalize())
}

fn ensure_generation(root: &Path) -> Result<(), MediaError> {
    let path = root.join(GENERATION_FILE);
    if !path.exists() {
        write_generation(root, 0)?;
    }
    Ok(())
}

fn read_generation(root: &Path) -> Result<u64, MediaError> {
    let value = fs::read_to_string(root.join(GENERATION_FILE))?;
    value
        .trim()
        .parse()
        .map_err(|error| MediaError::CacheManifest(format!("invalid cache generation: {error}")))
}

fn write_generation(root: &Path, generation: u64) -> Result<(), MediaError> {
    fs::create_dir_all(root)?;
    let mut temporary = NamedTempFile::new_in(root)?;
    writeln!(temporary, "{generation}")?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(root.join(GENERATION_FILE))
        .map_err(|error| MediaError::Io(error.error))?;
    sync_directory(root)
}

fn artifact_hasher(
    source_revision: &SourceRevision,
    variant: &VariantIdentity,
    dimensions: DisplayDimensions,
    native_detail: bool,
) -> Sha256 {
    let mut hasher = Sha256::new();
    hasher.update(b"oxy-media-artifact-v2\0");
    hasher.update(source_revision.revision_id.as_bytes());
    hasher.update(serde_json::to_vec(variant).unwrap_or_default());
    hasher.update(dimensions.width.to_le_bytes());
    hasher.update(dimensions.height.to_le_bytes());
    hasher.update([u8::from(native_detail)]);
    hasher
}

fn artifact_id(pending: &PendingArtifact) -> String {
    let mut hasher = artifact_hasher(
        &pending.source_revision,
        &pending.variant,
        pending.actual_dimensions,
        pending.native_detail,
    );
    hasher.update(&pending.bytes);
    format!("{:x}", hasher.finalize())
}

fn artifact_id_staged(pending: &PendingStagedArtifact) -> Result<String, MediaError> {
    use std::io::Read;
    let mut hasher = artifact_hasher(
        &pending.source_revision,
        &pending.variant,
        pending.actual_dimensions,
        pending.native_detail,
    );
    let mut file = File::open(&pending.staged_path)?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_source_revision(source: &SourceRevision) -> Result<(), MediaError> {
    if !is_lower_hex(&source.revision_id, 64) {
        return Err(MediaError::CacheArtifact(
            "invalid source revision identity".into(),
        ));
    }
    Ok(())
}

fn validate_extension(extension: &str) -> Result<(), MediaError> {
    if extension.is_empty()
        || extension.len() > 8
        || !extension
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(MediaError::CacheArtifact(
            "invalid artifact extension".into(),
        ));
    }
    Ok(())
}

fn dimensions_match(
    encoded: (u32, u32),
    display: DisplayDimensions,
    orientation: super::model::OrientationState,
) -> bool {
    encoded == (display.width, display.height)
        || (orientation == super::model::OrientationState::Metadata
            && encoded == (display.height, display.width))
}

fn retain_valid_artifacts(
    source_dir: &Path,
    artifacts: &mut Vec<ManifestArtifact>,
) -> Result<(), MediaError> {
    let mut first_error = None;
    artifacts.retain(
        |artifact| match valid_stored_artifact(source_dir, artifact) {
            Ok(valid) => valid,
            Err(error) => {
                first_error = Some(error);
                true
            }
        },
    );
    first_error.map_or(Ok(()), Err)
}

fn valid_stored_artifact(
    source_dir: &Path,
    artifact: &ManifestArtifact,
) -> Result<bool, MediaError> {
    let path = source_dir.join(&artifact.file_name);
    if path.parent() != Some(source_dir) {
        return Ok(false);
    }
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file() || metadata.len() != artifact.byte_size {
        return Ok(false);
    }
    let dimensions = match image::image_dimensions(&path) {
        Ok(dimensions) => dimensions,
        Err(image::ImageError::IoError(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(image::ImageError::IoError(error)) => return Err(error.into()),
        Err(_) => return Ok(false),
    };
    if !dimensions_match(
        dimensions,
        artifact.actual_dimensions,
        artifact.variant.presentation.orientation,
    ) {
        return Ok(false);
    }
    match validate_jpeg_completion(&path, &artifact.media_type) {
        Ok(()) => Ok(true),
        Err(MediaError::CacheArtifact(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

fn validate_jpeg_completion(path: &Path, media_type: &str) -> Result<(), MediaError> {
    if media_type != "image/jpeg" {
        return Ok(());
    }
    use std::io::{Read, Seek, SeekFrom};
    let mut file = File::open(path)?;
    if file.metadata()?.len() < 2 {
        return Err(MediaError::CacheArtifact("truncated JPEG artifact".into()));
    }
    file.seek(SeekFrom::End(-2))?;
    let mut marker = [0_u8; 2];
    file.read_exact(&mut marker)?;
    if marker != [0xff, 0xd9] {
        return Err(MediaError::CacheArtifact("truncated JPEG artifact".into()));
    }
    Ok(())
}

struct OwnedArtifact {
    path: PathBuf,
    size_bytes: u64,
    modified: SystemTime,
}

fn collect_artifacts(root: &Path) -> Result<Vec<OwnedArtifact>, MediaError> {
    let mut artifacts = Vec::new();
    collect_regular_files(&root.join(".backend-tmp"), &mut artifacts)?;
    for prefix in fs::read_dir(root)? {
        let prefix = prefix?;
        let prefix_name = prefix.file_name().to_string_lossy().into_owned();
        if !prefix.file_type()?.is_dir() || !is_lower_hex(&prefix_name, 2) {
            continue;
        }
        for source in fs::read_dir(prefix.path())? {
            let source = source?;
            let source_name = source.file_name().to_string_lossy().into_owned();
            if !source.file_type()?.is_dir()
                || !is_lower_hex(&source_name, 64)
                || !source_name.starts_with(&prefix_name)
            {
                continue;
            }
            collect_regular_files(&source.path().join(".tmp"), &mut artifacts)?;
            for artifact in fs::read_dir(source.path())? {
                let artifact = artifact?;
                let name = artifact.file_name();
                if !artifact.file_type()?.is_file() || name == MANIFEST_FILE {
                    continue;
                }
                push_owned_artifact(&artifact.path(), &mut artifacts)?;
            }
        }
    }
    Ok(artifacts)
}

fn collect_regular_files(
    directory: &Path,
    artifacts: &mut Vec<OwnedArtifact>,
) -> Result<(), MediaError> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_dir() {
        return Err(MediaError::CacheArtifact(format!(
            "owned cache directory is not a directory: {}",
            directory.display()
        )));
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            push_owned_artifact(&entry.path(), artifacts)?;
        }
    }
    Ok(())
}

fn push_owned_artifact(path: &Path, artifacts: &mut Vec<OwnedArtifact>) -> Result<(), MediaError> {
    let metadata = fs::symlink_metadata(path)?;
    artifacts.push(OwnedArtifact {
        path: path.to_owned(),
        size_bytes: metadata.len(),
        modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
    });
    Ok(())
}

fn repair_all_manifests(root: &Path, generation: u64) -> Result<(), MediaError> {
    for prefix in fs::read_dir(root)? {
        let prefix = prefix?;
        let prefix_name = prefix.file_name().to_string_lossy().into_owned();
        if !prefix.file_type()?.is_dir() || !is_lower_hex(&prefix_name, 2) {
            continue;
        }
        for source in fs::read_dir(prefix.path())? {
            let source = source?;
            let source_name = source.file_name().to_string_lossy().into_owned();
            if !source.file_type()?.is_dir()
                || !is_lower_hex(&source_name, 64)
                || !source_name.starts_with(&prefix_name)
            {
                continue;
            }
            let manifest_path = source.path().join(MANIFEST_FILE);
            let Ok(bytes) = fs::read(&manifest_path) else {
                continue;
            };
            let Ok(mut manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
                continue;
            };
            if manifest.cache_generation != generation {
                continue;
            }
            let original_len = manifest.artifacts.len();
            retain_valid_artifacts(&source.path(), &mut manifest.artifacts)?;
            if manifest.artifacts.len() == original_len {
                continue;
            }
            let mut temporary = NamedTempFile::new_in(source.path())?;
            serde_json::to_writer(&mut temporary, &manifest)
                .map_err(|error| MediaError::CacheManifest(error.to_string()))?;
            temporary.as_file().sync_all()?;
            temporary
                .persist(manifest_path)
                .map_err(|error| MediaError::Io(error.error))?;
        }
    }
    Ok(())
}

fn remove_owned_prefix(
    path: &Path,
    prefix_name: &str,
    protected: &[PathBuf],
) -> Result<(), MediaError> {
    for source in fs::read_dir(path)? {
        let source = source?;
        let name = source.file_name().to_string_lossy().into_owned();
        if source.file_type()?.is_dir() && is_lower_hex(&name, 64) && name.starts_with(prefix_name)
        {
            remove_owned_source(&source.path(), protected)?;
        }
    }
    remove_empty_directory(path)
}

fn remove_owned_source(path: &Path, protected: &[PathBuf]) -> Result<(), MediaError> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        if file_type.is_dir() && name == ".tmp" {
            remove_regular_files(&entry.path(), true, protected)?;
            remove_empty_directory(&entry.path())?;
        } else if file_type.is_file()
            && (name == MANIFEST_FILE
                || entry
                    .path()
                    .extension()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|extension| matches!(extension, "jpg" | "jpeg" | "png" | "webp")))
            && !protected.contains(&entry.path())
        {
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    remove_empty_directory(path)
}

fn remove_empty_directory(path: &Path) -> Result<(), MediaError> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), MediaError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> Result<(), MediaError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::model::{
        ArtifactPresentation, ArtifactRepresentation, ColorRequirement, ColorState,
        DetailRequirement, MEDIA_CACHE_POLICY_REVISION, OrientationRequirement, OrientationState,
        PresentationRequirement, RepresentationRequirement, SharpeningState,
    };
    use image::{DynamicImage, ImageFormat};
    use std::{io::Cursor, thread};

    fn fixture() -> (tempfile::TempDir, SourceRevision) {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.jpg");
        DynamicImage::new_rgb8(16, 12).save(&source).unwrap();
        let revision = SourceRevision::observe(&source).unwrap();
        (directory, revision)
    }

    fn jpeg(width: u32, height: u32) -> Arc<[u8]> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(width, height)
            .write_to(&mut bytes, ImageFormat::Jpeg)
            .unwrap();
        Arc::from(bytes.into_inner())
    }

    fn pending(source: &SourceRevision, target: u32, generation: u64) -> PendingArtifact {
        PendingArtifact {
            source_revision: source.clone(),
            variant: VariantIdentity {
                representation: ArtifactRepresentation::Decoded,
                presentation: ArtifactPresentation {
                    geometry: None,
                    orientation: OrientationState::Applied,
                    color: ColorState::Srgb,
                    sharpening: SharpeningState::None,
                },
                policy_revision: MEDIA_CACHE_POLICY_REVISION,
                target: target.to_string(),
            },
            actual_dimensions: DisplayDimensions {
                width: target,
                height: target / 2,
            },
            native_detail: false,
            media_type: "image/jpeg".into(),
            extension: "jpg".into(),
            bytes: jpeg(target, target / 2),
            cache_generation: generation,
        }
    }

    fn request(source: &SourceRevision, target: u32) -> CacheRequest {
        CacheRequest {
            source_revision: source.clone(),
            detail: DetailRequirement::Display {
                min_long_edge: target,
            },
            representation: RepresentationRequirement::AnyDisplay,
            presentation: PresentationRequirement {
                orientation: OrientationRequirement::Exact(OrientationState::Applied),
                color: ColorRequirement::Srgb,
                sharpening: SharpeningState::None,
            },
            policy_revision: MEDIA_CACHE_POLICY_REVISION,
            allow_interim: false,
        }
    }

    #[test]
    fn multiple_artifacts_coexist_and_lookup_chooses_closest_capability() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let generation = cache.generation().unwrap();
        cache.publish(pending(&source, 512, generation)).unwrap();
        cache.publish(pending(&source, 4096, generation)).unwrap();
        assert_eq!(cache.usage().unwrap().artifact_count, 2);
        let hit = cache.lookup(&request(&source, 256)).unwrap().unwrap();
        assert_eq!(hit.artifact.actual_dimensions.width, 512);
        let hit = cache.lookup(&request(&source, 2048)).unwrap().unwrap();
        assert_eq!(hit.artifact.actual_dimensions.width, 4096);
    }

    #[test]
    fn lookup_rereads_manifest_after_another_cache_instance_publishes() {
        let (directory, source) = fixture();
        let first = DiskMediaCache::new(directory.path(), 8).unwrap();
        let second = DiskMediaCache::new(directory.path(), 8).unwrap();
        assert!(first.lookup(&request(&source, 512)).unwrap().is_none());
        second
            .publish(pending(&source, 512, second.generation().unwrap()))
            .unwrap();
        assert!(first.lookup(&request(&source, 512)).unwrap().is_some());
    }

    #[test]
    fn restored_managed_path_is_bound_to_the_expected_projection_source() {
        let (directory, source_a) = fixture();
        let source_b_path = directory.path().join("source-b.jpg");
        DynamicImage::new_rgb8(16, 8).save(&source_b_path).unwrap();
        let source_b = SourceRevision::observe(&source_b_path).unwrap();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let publication = cache
            .publish(pending(&source_a, 512, cache.generation().unwrap()))
            .unwrap();
        let ArtifactLocation::Managed(path) = &publication.artifact.location else {
            unreachable!()
        };

        assert!(
            cache
                .validate_and_lease_path(path, &source_b)
                .unwrap()
                .is_none()
        );
        assert!(
            cache
                .validate_and_lease_path(path, &source_a)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn concurrent_publishers_merge_manifest_variants() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let generation = cache.generation().unwrap();
        let threads = [512, 1024, 2048, 4096]
            .into_iter()
            .map(|target| {
                let cache = cache.clone();
                let source = source.clone();
                thread::spawn(move || cache.publish(pending(&source, target, generation)).unwrap())
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(cache.usage().unwrap().artifact_count, 4);
    }

    #[test]
    fn corrupt_artifact_is_removed_from_manifest_and_treated_as_miss() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let artifact = cache
            .publish(pending(&source, 512, cache.generation().unwrap()))
            .unwrap();
        let artifact_id = artifact.artifact.artifact_id.clone();
        let ArtifactLocation::Managed(path) = artifact.artifact.location else {
            unreachable!()
        };
        fs::write(path, b"truncated").unwrap();
        assert!(cache.lookup(&request(&source, 512)).unwrap().is_none());
        let manifest = fs::read_to_string(cache.source_dir(&source).join(MANIFEST_FILE)).unwrap();
        assert!(!manifest.contains(&artifact_id));
    }

    #[test]
    fn restored_managed_path_requires_a_complete_manifest_artifact() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let publication = cache
            .publish(pending(&source, 512, cache.generation().unwrap()))
            .unwrap();
        let ArtifactLocation::Managed(path) = &publication.artifact.location else {
            unreachable!()
        };

        assert!(
            cache
                .validate_and_lease_path(path, &source)
                .unwrap()
                .is_some()
        );
        fs::write(path, b"truncated").unwrap();
        assert!(
            cache
                .validate_and_lease_path(path, &source)
                .unwrap()
                .is_none()
        );
        assert!(cache.lookup(&request(&source, 512)).unwrap().is_none());
    }

    #[test]
    fn clear_fences_old_publications_and_defers_leased_file_deletion() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let old_generation = cache.generation().unwrap();
        cache
            .publish(pending(&source, 512, old_generation))
            .unwrap();
        let hit = cache.lookup(&request(&source, 512)).unwrap().unwrap();
        let ArtifactLocation::Managed(path) = &hit.artifact.location else {
            unreachable!()
        };
        let path = path.clone();
        let second_process_view = DiskMediaCache::new(directory.path(), 8).unwrap();
        second_process_view.clear().unwrap();
        assert!(path.is_file());
        assert!(matches!(
            cache.publish(pending(&source, 4096, old_generation)),
            Err(MediaError::StaleCacheGeneration)
        ));
        assert!(cache.lookup(&request(&source, 512)).unwrap().is_none());
        drop(hit);
        second_process_view.clear().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn reopen_recovers_manifest_and_missing_artifact() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 1).unwrap();
        let artifact = cache
            .publish(pending(&source, 512, cache.generation().unwrap()))
            .unwrap();
        drop(cache);
        let reopened = DiskMediaCache::new(directory.path(), 1).unwrap();
        assert!(reopened.lookup(&request(&source, 512)).unwrap().is_some());
        let ArtifactLocation::Managed(path) = artifact.artifact.location else {
            unreachable!()
        };
        fs::remove_file(path).unwrap();
        assert!(reopened.lookup(&request(&source, 512)).unwrap().is_none());
    }

    #[test]
    fn publication_returns_transactional_lease_before_clear_can_delete_file() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let publication = cache
            .publish(pending(&source, 512, cache.generation().unwrap()))
            .unwrap();
        let ArtifactLocation::Managed(path) = &publication.artifact.location else {
            unreachable!()
        };
        let path = path.clone();
        let maintenance = DiskMediaCache::new(directory.path(), 8).unwrap();
        maintenance.clear().unwrap();
        assert!(path.is_file());
        assert!(cache.lookup(&request(&source, 512)).unwrap().is_none());
        drop(publication);
        maintenance.clear().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn generation_read_does_not_wait_for_the_publication_operation_mutex() {
        let (directory, _) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let expected = cache.generation().unwrap();
        let operation = cache.inner.operation.lock().unwrap();
        let reader = cache.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || sender.send(reader.generation()).unwrap());
        let observed = receiver.recv_timeout(Duration::from_secs(1));
        // Release before asserting so the old blocking implementation fails
        // the test without leaving a blocked worker behind.
        drop(operation);
        worker.join().unwrap();
        assert_eq!(observed.unwrap().unwrap(), expected);
    }

    #[test]
    fn generation_read_still_waits_for_the_cross_process_clear_lock() {
        let (directory, _) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let lock = cache.open_cache_lock().unwrap();
        FileExt::lock_exclusive(&lock).unwrap();
        let reader = cache.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || sender.send(reader.generation()).unwrap());
        let before_clear = receiver.recv_timeout(Duration::from_millis(50));
        write_generation(cache.root(), 7).unwrap();
        FileExt::unlock(&lock).unwrap();
        worker.join().unwrap();
        assert!(matches!(
            before_clear,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(receiver.recv().unwrap().unwrap(), 7);
    }

    #[test]
    fn prune_under_budget_does_not_rewrite_manifests() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let publication = cache
            .publish(pending(&source, 512, cache.generation().unwrap()))
            .unwrap();
        let manifest_path = cache.source_dir(&source).join(MANIFEST_FILE);
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        File::options()
            .write(true)
            .open(&manifest_path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let before = fs::metadata(&manifest_path).unwrap().modified().unwrap();
        let usage = cache.prune(u64::MAX).unwrap();
        assert_eq!(usage.artifact_count, 1);
        assert_eq!(usage.size_bytes, publication.artifact.byte_size);
        assert_eq!(
            fs::metadata(&manifest_path).unwrap().modified().unwrap(),
            before
        );
        assert!(cache.lookup(&request(&source, 512)).unwrap().is_some());
    }

    #[test]
    fn prune_does_not_rewrite_unchanged_leased_manifest_when_over_budget() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let publication = cache
            .publish(pending(&source, 512, cache.generation().unwrap()))
            .unwrap();
        let manifest_path = cache.source_dir(&source).join(MANIFEST_FILE);
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        File::options()
            .write(true)
            .open(&manifest_path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let before = fs::metadata(&manifest_path).unwrap().modified().unwrap();
        // The publication lease keeps this artifact alive even at a zero budget.
        let usage = cache.prune(0).unwrap();
        assert_eq!(usage.artifact_count, 1);
        assert_eq!(usage.size_bytes, publication.artifact.byte_size);
        assert_eq!(
            fs::metadata(&manifest_path).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn prune_preserves_active_lease_and_repairs_manifest() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let generation = cache.generation().unwrap();
        cache.publish(pending(&source, 512, generation)).unwrap();
        cache.publish(pending(&source, 1024, generation)).unwrap();
        let hit = cache.lookup(&request(&source, 512)).unwrap().unwrap();
        let usage = cache.prune(0).unwrap();
        assert_eq!(usage.artifact_count, 1);
        assert!(
            matches!(hit.artifact.location, ArtifactLocation::Managed(ref path) if path.is_file())
        );
    }

    #[test]
    fn independent_cache_instances_can_lease_the_same_artifact() {
        let (directory, source) = fixture();
        let first = DiskMediaCache::new(directory.path(), 8).unwrap();
        first
            .publish(pending(&source, 512, first.generation().unwrap()))
            .unwrap();
        let second = DiskMediaCache::new(directory.path(), 8).unwrap();
        let _first_hit = first.lookup(&request(&source, 512)).unwrap().unwrap();
        let _second_hit = second.lookup(&request(&source, 512)).unwrap().unwrap();
    }

    #[test]
    fn native_staged_file_transfers_ownership_without_copying() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let staging = cache.root().join(".backend-tmp");
        fs::create_dir(&staging).unwrap();
        let staged = staging.join("native-output.jpg");
        fs::write(&staged, jpeg(512, 256)).unwrap();
        let template = pending(&source, 512, cache.generation().unwrap());
        let artifact = cache
            .publish_staged(PendingStagedArtifact {
                source_revision: template.source_revision,
                variant: template.variant,
                actual_dimensions: template.actual_dimensions,
                native_detail: template.native_detail,
                media_type: template.media_type,
                extension: template.extension,
                staged_path: staged.clone(),
                cache_generation: template.cache_generation,
            })
            .unwrap();
        assert!(!staged.exists());
        assert!(
            matches!(artifact.artifact.location, ArtifactLocation::Managed(ref path) if path.is_file())
        );
    }

    #[test]
    fn atomic_miss_generation_is_rejected_after_clear() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let CacheLookup::Generate { cache_generation } =
            cache.lookup_or_generation(&request(&source, 512)).unwrap()
        else {
            panic!("empty cache unexpectedly hit")
        };
        cache.clear().unwrap();
        assert!(matches!(
            cache.publish(pending(&source, 512, cache_generation)),
            Err(MediaError::StaleCacheGeneration)
        ));
    }

    #[test]
    fn uppercase_source_revision_is_rejected_by_publication() {
        let (directory, mut source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        source.revision_id.replace_range(..1, "A");
        assert!(matches!(
            cache.publish(pending(&source, 512, cache.generation().unwrap())),
            Err(MediaError::CacheArtifact(_))
        ));
        assert_eq!(cache.usage().unwrap().artifact_count, 0);
    }

    #[test]
    fn source_change_rejects_publication_under_old_revision() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let generation = cache.generation().unwrap();
        fs::write(&source.canonical_path, b"replacement source").unwrap();
        assert!(matches!(
            cache.publish(pending(&source, 512, generation)),
            Err(MediaError::StaleSourceRevision)
        ));
        assert_eq!(cache.usage().unwrap().artifact_count, 0);
    }

    #[test]
    fn jpeg_missing_eoi_is_repaired_as_corrupt_even_when_header_remains() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let artifact = cache
            .publish(pending(&source, 512, cache.generation().unwrap()))
            .unwrap();
        let ArtifactLocation::Managed(path) = artifact.artifact.location else {
            unreachable!()
        };
        let length = fs::metadata(&path).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(length - 2)
            .unwrap();
        assert!(cache.lookup(&request(&source, 512)).unwrap().is_none());
    }

    #[test]
    fn clear_removes_only_fixed_owned_layout_and_staging() {
        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let backend_tmp = cache.root().join(".backend-tmp");
        fs::create_dir(&backend_tmp).unwrap();
        fs::write(backend_tmp.join("orphan"), b"orphan").unwrap();
        let source_tmp = cache.ensure_source_dir(&source).unwrap().join(".tmp");
        fs::create_dir(&source_tmp).unwrap();
        fs::write(source_tmp.join("orphan"), b"orphan").unwrap();
        let unrelated = cache.root().join("not-owned");
        fs::create_dir(&unrelated).unwrap();
        let sentinel = unrelated.join("sentinel");
        fs::write(&sentinel, b"keep").unwrap();

        cache.clear().unwrap();

        assert!(!backend_tmp.join("orphan").exists());
        assert!(!source_tmp.join("orphan").exists());
        assert!(sentinel.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_root_and_source_components_are_rejected_without_deletion() {
        use std::os::unix::fs::symlink;

        let cache_parent = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        let sentinel = victim.path().join("sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        symlink(victim.path(), cache_parent.path().join(CACHE_FOLDER)).unwrap();
        assert!(DiskMediaCache::new(cache_parent.path(), 8).is_err());
        assert!(sentinel.is_file());

        let (directory, source) = fixture();
        let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
        let prefix = cache.root().join(&source.revision_id[..2]);
        fs::create_dir(&prefix).unwrap();
        symlink(victim.path(), prefix.join(&source.revision_id)).unwrap();
        assert!(
            cache
                .publish(pending(&source, 512, cache.generation().unwrap()))
                .is_err()
        );
        assert!(sentinel.is_file());
    }
}
