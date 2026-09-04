use oxy_domain::{
    AssetKind, CaptureMetadata, EditableMetadata, FocusInfo, FocusRegion, MetadataProjection,
    ResourceLoadStatus,
};
use oxy_fs::sidecar_path;
use serde_json::Value;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};
use thiserror::Error;

mod engine;

pub use engine::{MetadataDocument, MetadataReader, NativeMetadataReader, RawMetadataTag};

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("ExifTool was not found; install it or set OXY_EXIFTOOL_PATH")]
    EmbeddedWorkerUnavailable,
    #[error("invalid rating {0}; expected 0 through 5")]
    InvalidRating(u8),
    #[error("Sony HIF supports only red, yellow, green, and blue color labels, not {0}")]
    UnsupportedHifColorLabel(String),
    #[error("metadata sidecar was not found for {0}")]
    SidecarUnavailable(PathBuf),
    #[error("metadata read failed: {0}")]
    Read(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Stable application boundary for metadata providers. All reads stay
/// in-process; the configured ExifTool executable is consulted only for an
/// explicit embedded write or as a batch compatibility fallback.
#[derive(Debug, Clone, Default)]
pub struct MetadataFacade {
    exiftool: Arc<RwLock<Option<PathBuf>>>,
    summary_cache: Arc<RwLock<HashMap<PathBuf, CachedSummaryMetadata>>>,
    next_observation: Arc<AtomicU64>,
    next_projection_revision: Arc<AtomicU64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedSummaryMetadata {
    fingerprint: SummaryMetadataFingerprint,
    valid_at: u64,
    projection_revision: u64,
    rating: Option<u8>,
    color_label: Option<String>,
    status: ResourceLoadStatus,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SummaryMetadataFingerprint {
    source_modified_at_ms: u64,
    source_size_bytes: u64,
    sidecar_modified_at_ns: Option<u128>,
    sidecar_size_bytes: Option<u64>,
    metadata_digest: Option<u64>,
}

/// Read lease bound to the source identity and logical time observed when the
/// request entered the coordinator, rather than when its worker completes.
#[derive(Debug, Clone)]
pub struct MetadataObservation {
    asset: oxy_domain::AssetSummary,
    fingerprint: SummaryMetadataFingerprint,
    valid_at: u64,
}

impl MetadataFacade {
    pub fn new(exiftool: Option<PathBuf>) -> Self {
        Self {
            exiftool: Arc::new(RwLock::new(exiftool)),
            summary_cache: Arc::new(RwLock::new(HashMap::new())),
            next_observation: Arc::new(AtomicU64::new(1)),
            next_projection_revision: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn set_exiftool(&self, executable: Option<PathBuf>) {
        *self
            .exiftool
            .write()
            .expect("metadata provider lock poisoned") = executable;
    }

    pub fn exiftool(&self) -> Option<PathBuf> {
        self.exiftool
            .read()
            .expect("metadata provider lock poisoned")
            .clone()
    }

    pub fn read_metadata(
        &self,
        path: &Path,
        kind: AssetKind,
    ) -> Result<EditableMetadata, MetadataError> {
        Ok(self.read_document(path, kind, None)?.editable)
    }

    /// Reads all normalized and raw metadata through one format-neutral parse.
    /// An adjacent sidecar overrides only the editable XMP projection.
    pub fn read_document(
        &self,
        path: &Path,
        _kind: AssetKind,
        display_dimensions: Option<(u32, u32)>,
    ) -> Result<MetadataDocument, MetadataError> {
        let mut document = NativeMetadataReader.read(path, display_dimensions)?;
        if sidecar_path(path).is_file() {
            document.editable = read_sidecar(path)?;
        }
        Ok(document)
    }

    /// Reads a document and publishes its editable projection into the same
    /// versioned state used by grid enrichment and metadata filtering.
    pub fn read_document_for_asset(
        &self,
        asset: &oxy_domain::AssetSummary,
        display_dimensions: Option<(u32, u32)>,
    ) -> Result<(MetadataDocument, MetadataProjection), MetadataError> {
        let observation = self.observe_asset(asset);
        self.read_document_observation(&observation, display_dimensions)
    }

    pub fn observe_asset(&self, asset: &oxy_domain::AssetSummary) -> MetadataObservation {
        self.observe_asset_at(asset, self.begin_observation())
    }

    pub fn observe_asset_at(
        &self,
        asset: &oxy_domain::AssetSummary,
        valid_at: u64,
    ) -> MetadataObservation {
        MetadataObservation {
            asset: asset.clone(),
            fingerprint: summary_metadata_fingerprint(asset),
            valid_at,
        }
    }

    pub fn restore_projection(
        &self,
        observation: &MetadataObservation,
        projection: &MetadataProjection,
    ) {
        self.next_projection_revision.fetch_max(
            projection.projection_revision.saturating_add(1),
            Ordering::Relaxed,
        );
        self.summary_cache
            .write()
            .expect("metadata summary cache lock poisoned")
            .insert(
                observation.asset.path.clone(),
                CachedSummaryMetadata {
                    fingerprint: observation.fingerprint,
                    valid_at: projection.valid_at,
                    projection_revision: projection.projection_revision,
                    rating: projection.rating,
                    color_label: projection.color_label.clone(),
                    status: projection.status,
                    error: projection.error.clone(),
                },
            );
    }

    pub fn read_document_observation(
        &self,
        observation: &MetadataObservation,
        display_dimensions: Option<(u32, u32)>,
    ) -> Result<(MetadataDocument, MetadataProjection), MetadataError> {
        let document = self.read_document(
            &observation.asset.path,
            observation.asset.kind,
            display_dimensions,
        )?;
        let projection = self.merge_summary_observation(
            &observation.asset,
            observation.fingerprint,
            observation.valid_at,
            document.editable.rating,
            document.editable.color_label.clone(),
        );
        Ok((document, projection))
    }

    pub fn read_summary_observation(
        &self,
        observation: &MetadataObservation,
    ) -> Result<MetadataProjection, MetadataError> {
        self.read_document_observation(observation, None)
            .map(|(_, projection)| projection)
    }

    pub fn mark_observation_loading(
        &self,
        observation: &MetadataObservation,
    ) -> MetadataProjection {
        self.transition_observation(observation, ResourceLoadStatus::Loading, None)
    }

    pub fn fail_observation(
        &self,
        observation: &MetadataObservation,
        error: String,
    ) -> MetadataProjection {
        self.transition_observation(observation, ResourceLoadStatus::Error, Some(error))
    }

    fn transition_observation(
        &self,
        observation: &MetadataObservation,
        status: ResourceLoadStatus,
        error: Option<String>,
    ) -> MetadataProjection {
        let mut cache = self
            .summary_cache
            .write()
            .expect("metadata summary cache lock poisoned");
        if let Some(current) = cache
            .get(&observation.asset.path)
            .filter(|current| current.valid_at > observation.valid_at)
        {
            return projection_from_cache(&observation.asset, current);
        }
        let (rating, color_label) = cache
            .get(&observation.asset.path)
            .filter(|current| current.fingerprint == observation.fingerprint)
            .map(|current| (current.rating, current.color_label.clone()))
            .unwrap_or_default();
        let projection_revision = self
            .next_projection_revision
            .fetch_add(1, Ordering::Relaxed);
        cache.insert(
            observation.asset.path.clone(),
            CachedSummaryMetadata {
                fingerprint: observation.fingerprint,
                valid_at: observation.valid_at,
                projection_revision,
                rating,
                color_label,
                status,
                error,
            },
        );
        projection_from_cache(
            &observation.asset,
            cache
                .get(&observation.asset.path)
                .expect("inserted projection transition"),
        )
    }

    pub fn enrich_summaries(
        &self,
        assets: &mut [oxy_domain::AssetSummary],
    ) -> Result<(), MetadataError> {
        self.enrich_summaries_with_projections(assets).map(|_| ())
    }

    /// Enriches summaries and returns the accepted authoritative projection
    /// for every asset in input order.
    pub fn enrich_summaries_with_projections(
        &self,
        assets: &mut [oxy_domain::AssetSummary],
    ) -> Result<Vec<MetadataProjection>, MetadataError> {
        let fingerprints = assets
            .iter()
            .map(summary_metadata_fingerprint)
            .collect::<Vec<_>>();
        let cached = self
            .summary_cache
            .read()
            .expect("metadata summary cache lock poisoned");
        let mut misses = Vec::new();
        let mut projections = vec![None; assets.len()];
        for (index, asset) in assets.iter_mut().enumerate() {
            match cached.get(&asset.path) {
                Some(entry)
                    if entry.fingerprint == fingerprints[index]
                        && entry.status == ResourceLoadStatus::Ready =>
                {
                    asset.rating = entry.rating;
                    asset.color_label.clone_from(&entry.color_label);
                    projections[index] = Some(projection_from_cache(asset, entry));
                }
                _ => misses.push((index, asset.clone(), self.begin_observation())),
            }
        }
        drop(cached);

        if misses.is_empty() {
            return Ok(projections.into_iter().flatten().collect());
        }

        let executable = self.exiftool();
        let mut uncached_assets = misses
            .iter()
            .map(|(_, asset, _)| asset.clone())
            .collect::<Vec<_>>();
        enrich_summaries_with_exiftool(&mut uncached_assets, executable.as_deref())?;

        for ((index, original, valid_at), enriched) in misses.into_iter().zip(uncached_assets) {
            let projection = self.merge_summary_observation(
                &original,
                fingerprints[index],
                valid_at,
                enriched.rating,
                enriched.color_label,
            );
            projections[index] = Some(projection);
            if let Some(current) = self
                .summary_cache
                .read()
                .expect("metadata summary cache lock poisoned")
                .get(&original.path)
                .filter(|entry| entry.fingerprint == fingerprints[index])
            {
                assets[index].rating = current.rating;
                assets[index].color_label.clone_from(&current.color_label);
            }
        }
        Ok(projections.into_iter().flatten().collect())
    }

    fn begin_observation(&self) -> u64 {
        self.next_observation.fetch_add(1, Ordering::Relaxed)
    }

    fn merge_summary_observation(
        &self,
        asset: &oxy_domain::AssetSummary,
        fingerprint: SummaryMetadataFingerprint,
        valid_at: u64,
        rating: Option<u8>,
        color_label: Option<String>,
    ) -> MetadataProjection {
        let mut cache = self
            .summary_cache
            .write()
            .expect("metadata summary cache lock poisoned");
        if let Some(current) = cache
            .get(&asset.path)
            .filter(|current| current.valid_at > valid_at)
        {
            return projection_from_cache(asset, current);
        }
        let projection_revision = self
            .next_projection_revision
            .fetch_add(1, Ordering::Relaxed);
        cache.insert(
            asset.path.clone(),
            CachedSummaryMetadata {
                fingerprint,
                valid_at,
                projection_revision,
                rating,
                color_label,
                status: ResourceLoadStatus::Ready,
                error: None,
            },
        );
        projection_from_cache(asset, cache.get(&asset.path).expect("inserted projection"))
    }

    pub fn cached_projection(
        &self,
        asset: &oxy_domain::AssetSummary,
    ) -> Option<MetadataProjection> {
        let fingerprint = summary_metadata_fingerprint(asset);
        self.summary_cache
            .read()
            .expect("metadata summary cache lock poisoned")
            .get(&asset.path)
            .filter(|entry| entry.fingerprint == fingerprint)
            .map(|entry| projection_from_cache(asset, entry))
    }

    /// Drops cached rating/color projections for one directory. The cache is
    /// shared by grid enrichment and metadata-aware filtering.
    pub fn invalidate_summary_directory(&self, directory: &Path) {
        self.summary_cache
            .write()
            .expect("metadata summary cache lock poisoned")
            .retain(|path, _| path.parent() != Some(directory));
    }

    pub fn patch_metadata(
        &self,
        path: &Path,
        kind: AssetKind,
        patch: &oxy_domain::MetadataPatch,
    ) -> Result<PathBuf, MetadataError> {
        let sidecar = patch_metadata_to_sidecar(path, kind, patch)?;
        self.summary_cache
            .write()
            .expect("metadata summary cache lock poisoned")
            .remove(path);
        Ok(sidecar)
    }

    pub fn sync_metadata_to_embedded(&self, path: &Path) -> Result<PathBuf, MetadataError> {
        if !sidecar_path(path).is_file() {
            return Err(MetadataError::SidecarUnavailable(path.to_path_buf()));
        }
        let metadata = read_sidecar(path)?;
        let patch = oxy_domain::MetadataPatch {
            rating: Some(metadata.rating),
            color_label: Some(metadata.color_label),
            ..oxy_domain::MetadataPatch::default()
        };
        let executable = self.exiftool();
        let updated = patch_embedded(path, &patch, executable.as_deref())?;
        self.summary_cache
            .write()
            .expect("metadata summary cache lock poisoned")
            .remove(path);
        Ok(updated)
    }

    pub fn exiftool_version(&self) -> Result<String, MetadataError> {
        let executable = self.exiftool();
        probe_exiftool(executable.as_deref())
    }
}

fn summary_metadata_fingerprint(asset: &oxy_domain::AssetSummary) -> SummaryMetadataFingerprint {
    let sidecar = sidecar_path(&asset.path);
    let sidecar_metadata = fs::metadata(&sidecar).ok();
    let metadata_digest = if sidecar_metadata.is_some() {
        fs::read(&sidecar)
            .ok()
            .map(|bytes| stable_bytes_digest(&bytes))
    } else if is_sony_hif(&asset.path) {
        engine::heif_metadata_digest(&asset.path)
    } else {
        None
    };
    SummaryMetadataFingerprint {
        source_modified_at_ms: asset.modified_at_ms,
        source_size_bytes: asset.size_bytes,
        sidecar_modified_at_ns: sidecar_metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos()),
        sidecar_size_bytes: sidecar_metadata.map(|metadata| metadata.len()),
        metadata_digest,
    }
}

pub fn metadata_source_revision(asset: &oxy_domain::AssetSummary) -> String {
    source_revision_from_fingerprint(summary_metadata_fingerprint(asset))
}

fn source_revision_from_fingerprint(fingerprint: SummaryMetadataFingerprint) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        fingerprint.source_modified_at_ms,
        fingerprint.source_size_bytes,
        fingerprint.sidecar_modified_at_ns.unwrap_or_default(),
        fingerprint.sidecar_size_bytes.unwrap_or_default(),
        fingerprint.metadata_digest.unwrap_or_default()
    )
}

fn stable_bytes_digest(bytes: &[u8]) -> u64 {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 prefix is eight bytes"),
    )
}

fn projection_from_cache(
    asset: &oxy_domain::AssetSummary,
    cached: &CachedSummaryMetadata,
) -> MetadataProjection {
    MetadataProjection {
        path: asset.path.clone(),
        source_revision: source_revision_from_fingerprint(cached.fingerprint),
        projection_revision: cached.projection_revision,
        valid_at: cached.valid_at,
        status: cached.status,
        rating: cached.rating,
        color_label: cached.color_label.clone(),
        error: cached.error.clone(),
    }
}

/// Reads editable metadata from an adjacent XMP sidecar first. Without a
/// sidecar, all supported formats, including RAW, try embedded XMP.
pub fn read_metadata(path: &Path, kind: AssetKind) -> Result<EditableMetadata, MetadataError> {
    read_metadata_with_exiftool(path, kind, None)
}

/// Compatibility entry point retained for callers that previously selected an
/// ExifTool executable. Reads now always use the native unified engine.
pub fn read_metadata_with_exiftool(
    path: &Path,
    kind: AssetKind,
    exiftool: Option<&Path>,
) -> Result<EditableMetadata, MetadataError> {
    let _ = exiftool;
    Ok(MetadataFacade::default()
        .read_document(path, kind, None)?
        .editable)
}

/// Enriches summaries in-place. A single ExifTool process is used per chunk so
/// metadata filtering does not spawn a worker for every JPEG/HEIF file.
pub fn enrich_summaries(assets: &mut [oxy_domain::AssetSummary]) -> Result<(), MetadataError> {
    enrich_summaries_with_exiftool(assets, None)
}

pub fn enrich_summaries_with_exiftool(
    assets: &mut [oxy_domain::AssetSummary],
    exiftool: Option<&Path>,
) -> Result<(), MetadataError> {
    let embedded = assets
        .iter()
        .filter(|asset| !sidecar_path(&asset.path).is_file())
        .map(|asset| asset.path.clone())
        .collect::<Vec<_>>();
    let embedded_values = match read_embedded_batch(&embedded, exiftool) {
        Ok(values) => values,
        Err(MetadataError::EmbeddedWorkerUnavailable) => HashMap::new(),
        Err(error) => return Err(error),
    };
    for asset in assets {
        let metadata = if sidecar_path(&asset.path).is_file() {
            read_sidecar(&asset.path)?
        } else {
            embedded_values
                .get(&asset.path)
                .cloned()
                .unwrap_or_default()
        };
        asset.rating = metadata.rating;
        asset.color_label = metadata.color_label;
    }
    Ok(())
}

pub fn patch_metadata(
    path: &Path,
    kind: AssetKind,
    patch: &oxy_domain::MetadataPatch,
) -> Result<PathBuf, MetadataError> {
    patch_metadata_to_sidecar(path, kind, patch)
}

pub fn patch_metadata_to_sidecar(
    path: &Path,
    _kind: AssetKind,
    patch: &oxy_domain::MetadataPatch,
) -> Result<PathBuf, MetadataError> {
    validate_patch(patch)?;
    patch_sidecar(path, patch)
}

fn validate_patch(patch: &oxy_domain::MetadataPatch) -> Result<(), MetadataError> {
    if let Some(Some(rating)) = patch.rating
        && rating > 5
    {
        return Err(MetadataError::InvalidRating(rating));
    }
    Ok(())
}

fn read_sidecar(asset_path: &Path) -> Result<EditableMetadata, MetadataError> {
    let path = sidecar_path(asset_path);
    if !path.is_file() {
        return Ok(EditableMetadata::default());
    }
    let xml = fs::read_to_string(path)?;
    Ok(EditableMetadata {
        rating: xmp_value(&xml, "Rating").and_then(|value| value.parse().ok()),
        color_label: xmp_value(&xml, "Label").filter(|value| !value.is_empty()),
        ..EditableMetadata::default()
    })
}

pub(crate) fn xmp_value(xml: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let attribute = format!("xmp:{name}={quote}");
        if let Some(start) = xml.find(&attribute) {
            let value = &xml[start + attribute.len()..];
            return value.find(quote).map(|end| unescape_xml(&value[..end]));
        }
    }
    let open = format!("<xmp:{name}>");
    let close = format!("</xmp:{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(unescape_xml(xml[start..end].trim()))
}

fn unescape_xml(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn read_embedded_batch(
    paths: &[PathBuf],
    exiftool: Option<&Path>,
) -> Result<HashMap<PathBuf, EditableMetadata>, MetadataError> {
    let mut result = HashMap::new();
    for paths in paths.chunks(64) {
        if paths.is_empty() {
            continue;
        }
        let mut exiftool_paths = Vec::with_capacity(paths.len());
        for path in paths {
            match NativeMetadataReader.read(path, None) {
                Ok(document) => {
                    result.insert(path.clone(), document.editable);
                }
                Err(_) => exiftool_paths.push(path),
            }
        }
        if exiftool_paths.is_empty() {
            continue;
        }
        let mut command = exiftool_command(exiftool);
        command.args([
            "-json",
            "-n",
            "-XMP:Rating",
            "-XMP:Label",
            "-XMP:Title",
            "-XMP:Description",
            "-XMP:Creator",
            "-XMP:Copyright",
            "-XMP:Subject",
        ]);
        command.args(&exiftool_paths);
        let output = exiftool_output(&mut command)?;
        if !output.status.success() {
            return Err(MetadataError::Read(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        let rows: Vec<Value> = serde_json::from_slice(&output.stdout)
            .map_err(|error| MetadataError::Read(error.to_string()))?;
        for (index, row) in rows.into_iter().enumerate() {
            let Some(source) = row.get("SourceFile").and_then(Value::as_str) else {
                continue;
            };
            let metadata = metadata_from_json(&row);
            result.insert(PathBuf::from(source), metadata.clone());
            // ExifTool renders Windows paths with forward slashes. Keep the
            // original input spelling as an alias so Unicode drive paths and
            // separator normalization cannot make the metadata lookup miss.
            if let Some(path) = exiftool_paths.get(index) {
                result.insert((*path).clone(), metadata);
            }
        }
    }
    Ok(result)
}

fn is_sony_hif(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("hif"))
}

fn metadata_from_json(row: &Value) -> EditableMetadata {
    EditableMetadata {
        rating: row
            .get("Rating")
            .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
            .and_then(|value| u8::try_from(value).ok()),
        color_label: string_value(row.get("Label")).and_then(normalize_color_label),
        title: string_value(row.get("Title")),
        description: string_value(row.get("Description")),
        creator: string_value(row.get("Creator")),
        copyright: string_value(row.get("Copyright")),
        keywords: match row.get("Subject") {
            Some(Value::Array(values)) => values
                .iter()
                .filter_map(|value| string_value(Some(value)))
                .collect(),
            value => string_value(value).into_iter().collect(),
        },
    }
}

pub(crate) fn normalize_color_label(value: String) -> Option<String> {
    if value.eq_ignore_ascii_case("none") {
        return None;
    }
    ["Red", "Yellow", "Green", "Blue", "Purple"]
        .into_iter()
        .find(|label| label.eq_ignore_ascii_case(&value))
        .map(str::to_owned)
        .or(Some(value))
}

fn string_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Array(values) => values.first().and_then(|value| string_value(Some(value))),
        _ => None,
    }
}

fn patch_embedded(
    path: &Path,
    patch: &oxy_domain::MetadataPatch,
    exiftool: Option<&Path>,
) -> Result<PathBuf, MetadataError> {
    let mut command = exiftool_command(exiftool);
    command.args(["-overwrite_original", "-P"]);
    let sony_hif = is_sony_hif(path);
    if sony_hif {
        // Imaging Edge Viewer expects its HIF rating fields in shorthand XMP
        // and uses lowercase color names plus explicit zero/None sentinels.
        command.args(["-api", "Compact=AllFormat"]);
    }
    add_patch_args(&mut command, patch, sony_hif)?;
    command.arg(path);
    let output = exiftool_output(&mut command)?;
    if !output.status.success() {
        return Err(MetadataError::Read(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(path.to_path_buf())
}

fn add_patch_args(
    command: &mut Command,
    patch: &oxy_domain::MetadataPatch,
    sony_hif: bool,
) -> Result<(), MetadataError> {
    if let Some(value) = patch.rating {
        let empty = if sony_hif { "0" } else { "" };
        command.arg(format!(
            "-XMP:Rating={}",
            value.map_or_else(|| empty.to_owned(), |value| value.to_string())
        ));
    }
    if let Some(value) = &patch.color_label {
        let value = if sony_hif {
            match value.as_deref() {
                None => "None",
                Some(value) if value.eq_ignore_ascii_case("red") => "red",
                Some(value) if value.eq_ignore_ascii_case("yellow") => "yellow",
                Some(value) if value.eq_ignore_ascii_case("green") => "green",
                Some(value) if value.eq_ignore_ascii_case("blue") => "blue",
                Some(value) => {
                    return Err(MetadataError::UnsupportedHifColorLabel(value.to_owned()));
                }
            }
        } else {
            value.as_deref().unwrap_or_default()
        };
        command.arg(format!("-XMP:Label={value}"));
    }
    Ok(())
}

fn exiftool_command(configured: Option<&Path>) -> Command {
    let executable = configured
        .map(Path::as_os_str)
        .map(OsString::from)
        .or_else(|| std::env::var_os("OXY_EXIFTOOL_PATH"))
        .unwrap_or_else(|| OsString::from("exiftool"));
    let mut command = platform_exiftool_command(executable);
    command.env("LC_ALL", "C").env("LANG", "C");
    command
}

/// Validates the selected provider and returns its version string.
pub fn probe_exiftool(executable: Option<&Path>) -> Result<String, MetadataError> {
    let output = exiftool_output(exiftool_command(executable).arg("-ver"))?;
    if !output.status.success() {
        return Err(MetadataError::Read(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if version.is_empty() {
        return Err(MetadataError::Read(
            "ExifTool returned an empty version".into(),
        ));
    }
    Ok(version)
}

#[cfg(target_os = "windows")]
fn platform_exiftool_command(executable: OsString) -> Command {
    let mut command = Command::new(executable);
    // The packaged application uses the Windows GUI subsystem, but ExifTool is
    // a console executable. Without this flag Windows briefly creates a console
    // window whenever metadata is read or written.
    command.creation_flags(0x0800_0000);
    command
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn platform_exiftool_command(executable: OsString) -> Command {
    Command::new(executable)
}

fn exiftool_output(command: &mut Command) -> Result<Output, MetadataError> {
    command.output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            MetadataError::EmbeddedWorkerUnavailable
        } else {
            MetadataError::Io(error)
        }
    })
}

fn patch_sidecar(
    asset_path: &Path,
    patch: &oxy_domain::MetadataPatch,
) -> Result<PathBuf, MetadataError> {
    let destination = sidecar_path(asset_path);
    let mut xml = if destination.is_file() {
        fs::read_to_string(&destination)?
    } else {
        let embedded = NativeMetadataReader
            .read(asset_path, None)
            .map(|document| document.editable)
            .unwrap_or_default();
        serialize_xmp(&embedded)
    };
    if let Some(value) = patch.rating {
        xml = set_xmp_attribute(
            &xml,
            "Rating",
            value.map(|value| value.to_string()).as_deref(),
        )?;
    }
    if let Some(value) = &patch.color_label {
        xml = set_xmp_attribute(&xml, "Label", value.as_deref())?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&destination)?;
    file.write_all(xml.as_bytes())?;
    file.sync_all()?;
    Ok(destination)
}

fn set_xmp_attribute(xml: &str, name: &str, value: Option<&str>) -> Result<String, MetadataError> {
    let start = xml
        .find("<rdf:Description")
        .ok_or_else(|| MetadataError::Read("XMP has no rdf:Description element".into()))?;
    let end = xml[start..]
        .find('>')
        .map(|offset| start + offset)
        .ok_or_else(|| MetadataError::Read("XMP rdf:Description is not terminated".into()))?;
    let mut opening = xml[start..end].to_owned();
    let needle = format!(" xmp:{name}=\"");
    if let Some(attribute_start) = opening.find(&needle) {
        let value_start = attribute_start + needle.len();
        let value_end = opening[value_start..]
            .find('"')
            .map(|offset| value_start + offset)
            .ok_or_else(|| MetadataError::Read(format!("invalid xmp:{name} attribute")))?;
        opening.replace_range(attribute_start..=value_end, "");
        if let Some(value) = value {
            insert_description_attribute(&mut opening, name, value);
        }
        return Ok(format!("{}{}{}", &xml[..start], opening, &xml[end..]));
    }

    let element_open = format!("<xmp:{name}>");
    let element_close = format!("</xmp:{name}>");
    if let Some(element_start) = xml.find(&element_open)
        && let Some(relative_end) = xml[element_start + element_open.len()..].find(&element_close)
    {
        let content_start = element_start + element_open.len();
        let element_end = content_start + relative_end + element_close.len();
        let replacement = value
            .map(|value| format!("{element_open}{}{element_close}", escape_xml(value)))
            .unwrap_or_default();
        return Ok(format!(
            "{}{}{}",
            &xml[..element_start],
            replacement,
            &xml[element_end..]
        ));
    }

    if let Some(value) = value {
        insert_description_attribute(&mut opening, name, value);
    }
    Ok(format!("{}{}{}", &xml[..start], opening, &xml[end..]))
}

fn insert_description_attribute(opening: &mut String, name: &str, value: &str) {
    let self_closing_insertion = opening
        .trim_end()
        .strip_suffix('/')
        .map(|without_slash| without_slash.len());
    let insertion = self_closing_insertion.unwrap_or(opening.len());
    let trailing_space = if self_closing_insertion.is_some() {
        " "
    } else {
        ""
    };
    let attribute = format!(" xmp:{name}=\"{}\"{trailing_space}", escape_xml(value));
    opening.insert_str(insertion, &attribute);
}

/// Reads shooting focus information through the format-neutral native engine.
pub fn read_focus_info(
    path: &Path,
    display_dimensions: Option<(u32, u32)>,
) -> Result<Option<FocusInfo>, MetadataError> {
    Ok(NativeMetadataReader.read(path, display_dimensions)?.focus)
}

/// Reads immutable shooting values and focus data in one in-process,
/// format-neutral parse. This does not depend on the optional ExifTool worker.
pub fn read_capture_details(
    path: &Path,
    display_dimensions: Option<(u32, u32)>,
) -> Result<(CaptureMetadata, Option<FocusInfo>), MetadataError> {
    let document = NativeMetadataReader.read(path, display_dimensions)?;
    Ok((document.capture, document.focus))
}

pub(crate) fn format_number(value: f64) -> String {
    if (value - value.round()).abs() < 0.005 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}").trim_end_matches('0').to_owned()
    }
}

pub(crate) fn orient_focus_info(
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    frame_size: Option<(u32, u32)>,
    orientation: u16,
) -> FocusInfo {
    let (coordinate_width, coordinate_height, center_x, center_y, swap_frame_axes) =
        match orientation {
            2 => (width, height, width.saturating_sub(x), y, false),
            3 => (
                width,
                height,
                width.saturating_sub(x),
                height.saturating_sub(y),
                false,
            ),
            4 => (width, height, x, height.saturating_sub(y), false),
            5 => (height, width, y, x, true),
            6 => (height, width, height.saturating_sub(y), x, true),
            7 => (
                height,
                width,
                height.saturating_sub(y),
                width.saturating_sub(x),
                true,
            ),
            8 => (height, width, y, width.saturating_sub(x), true),
            _ => (width, height, x, y, false),
        };
    let (frame_width, frame_height) = frame_size
        .map(|(frame_width, frame_height)| {
            if swap_frame_axes {
                (frame_height, frame_width)
            } else {
                (frame_width, frame_height)
            }
        })
        .unzip();
    FocusInfo {
        coordinate_width,
        coordinate_height,
        regions: vec![FocusRegion {
            center_x,
            center_y,
            width: frame_width,
            height: frame_height,
        }],
    }
}

pub fn write_metadata(
    path: &Path,
    _kind: AssetKind,
    metadata: &EditableMetadata,
) -> Result<PathBuf, MetadataError> {
    write_sidecar(path, metadata)
}

pub fn write_raw_sidecar(
    raw_path: &Path,
    metadata: &EditableMetadata,
) -> Result<PathBuf, MetadataError> {
    write_sidecar(raw_path, metadata)
}

pub fn write_sidecar(
    asset_path: &Path,
    metadata: &EditableMetadata,
) -> Result<PathBuf, MetadataError> {
    let destination = sidecar_path(asset_path);
    let temporary = destination.with_extension("xmp.oxy-tmp");
    let xml = serialize_xmp(metadata);
    let mut file = fs::File::create(&temporary)?;
    file.write_all(xml.as_bytes())?;
    file.sync_all()?;
    fs::rename(temporary, &destination)?;
    Ok(destination)
}

fn serialize_xmp(metadata: &EditableMetadata) -> String {
    let keywords = metadata
        .keywords
        .iter()
        .map(|value| format!("<rdf:li>{}</rdf:li>", escape_xml(value)))
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" xmp:Rating="{rating}" xmp:Label="{label}" photoshop:Credit="{creator}">
      <dc:title><rdf:Alt><rdf:li xml:lang="x-default">{title}</rdf:li></rdf:Alt></dc:title>
      <dc:description><rdf:Alt><rdf:li xml:lang="x-default">{description}</rdf:li></rdf:Alt></dc:description>
      <dc:rights><rdf:Alt><rdf:li xml:lang="x-default">{copyright}</rdf:li></rdf:Alt></dc:rights>
      <dc:subject><rdf:Bag>{keywords}</rdf:Bag></dc:subject>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#,
        rating = metadata.rating.unwrap_or_default(),
        label = escape_xml(metadata.color_label.as_deref().unwrap_or_default()),
        creator = escape_xml(metadata.creator.as_deref().unwrap_or_default()),
        title = escape_xml(metadata.title.as_deref().unwrap_or_default()),
        description = escape_xml(metadata.description.as_deref().unwrap_or_default()),
        copyright = escape_xml(metadata.copyright.as_deref().unwrap_or_default()),
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn minimal_raw_with_xmp(rating: u8, label: &str) -> Vec<u8> {
        let xmp = format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="{rating}" xmp:Label="{label}" /></rdf:RDF></x:xmpmeta>"#
        );
        let xmp = xmp.as_bytes();
        let xmp_offset = 8 + 2 + 12 + 4;
        let mut bytes = Vec::with_capacity(xmp_offset + xmp.len());
        bytes.extend_from_slice(b"II");
        bytes.extend_from_slice(&42_u16.to_le_bytes());
        bytes.extend_from_slice(&8_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&0x02bc_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&(xmp.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(xmp_offset as u32).to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(xmp);
        bytes
    }

    #[test]
    fn facade_switches_the_embedded_provider_without_affecting_its_contract() {
        let facade = MetadataFacade::default();
        assert_eq!(facade.exiftool(), None);
        let configured = PathBuf::from("/managed/exiftool");
        facade.set_exiftool(Some(configured.clone()));
        assert_eq!(facade.exiftool(), Some(configured));
    }

    #[test]
    fn facade_shares_summary_metadata_cache_across_callers() {
        let directory = tempdir().unwrap();
        let raw = directory.path().join("photo.nef");
        fs::write(&raw, b"camera image bytes").unwrap();
        patch_metadata(
            &raw,
            AssetKind::Raw,
            &oxy_domain::MetadataPatch {
                rating: Some(Some(5)),
                color_label: Some(Some("Blue".into())),
                ..oxy_domain::MetadataPatch::default()
            },
        )
        .unwrap();
        let mut assets =
            oxy_fs::scan_directory(directory.path(), &oxy_domain::AssetQuery::default(), 0)
                .unwrap()
                .items;
        let facade = MetadataFacade::default();

        facade.enrich_summaries(&mut assets).unwrap();
        assert_eq!(assets[0].rating, Some(5));
        assert_eq!(assets[0].color_label.as_deref(), Some("Blue"));
        assert_eq!(facade.summary_cache.read().unwrap().len(), 1);

        let shared_caller = facade.clone();
        assets[0].rating = None;
        assets[0].color_label = None;
        shared_caller.enrich_summaries(&mut assets).unwrap();
        assert_eq!(assets[0].rating, Some(5));
        assert_eq!(assets[0].color_label.as_deref(), Some("Blue"));
        assert!(Arc::ptr_eq(
            &facade.summary_cache,
            &shared_caller.summary_cache
        ));

        patch_metadata(
            &raw,
            AssetKind::Raw,
            &oxy_domain::MetadataPatch {
                rating: Some(Some(2)),
                color_label: Some(Some("Purple".into())),
                ..oxy_domain::MetadataPatch::default()
            },
        )
        .unwrap();
        shared_caller.enrich_summaries(&mut assets).unwrap();
        assert_eq!(assets[0].rating, Some(2));
        assert_eq!(assets[0].color_label.as_deref(), Some("Purple"));

        shared_caller.invalidate_summary_directory(directory.path());
        assert!(facade.summary_cache.read().unwrap().is_empty());
    }

    #[test]
    fn older_observation_cannot_overwrite_a_newer_projection() {
        let directory = tempdir().unwrap();
        let raw = directory.path().join("photo.nef");
        fs::write(&raw, b"camera image bytes").unwrap();
        let asset = oxy_fs::scan_directory(directory.path(), &oxy_domain::AssetQuery::default(), 0)
            .unwrap()
            .items
            .remove(0);
        let facade = MetadataFacade::default();
        let old_valid_at = facade.begin_observation();
        let new_valid_at = facade.begin_observation();
        let fingerprint = summary_metadata_fingerprint(&asset);

        let selected = facade.merge_summary_observation(
            &asset,
            fingerprint,
            new_valid_at,
            Some(5),
            Some("Red".into()),
        );
        let late_background = facade.merge_summary_observation(
            &asset,
            fingerprint,
            old_valid_at,
            Some(1),
            Some("Blue".into()),
        );

        assert_eq!(
            late_background.projection_revision,
            selected.projection_revision
        );
        assert_eq!(late_background.rating, Some(5));
        assert_eq!(late_background.color_label.as_deref(), Some("Red"));
    }

    #[test]
    fn hif_source_revision_tracks_embedded_xmp_when_file_stat_is_unchanged() {
        let directory = tempdir().unwrap();
        let hif = directory.path().join("photo.HIF");
        let content = |rating| {
            format!(
                "....ftypSHIF....<x:xmpmeta><rdf:RDF><rdf:Description xmp:Rating='{rating}' /></rdf:RDF></x:xmpmeta>"
            )
        };
        fs::write(&hif, content(1)).unwrap();
        let asset = oxy_fs::scan_directory(directory.path(), &oxy_domain::AssetQuery::default(), 0)
            .unwrap()
            .items
            .remove(0);
        let first = metadata_source_revision(&asset);

        fs::write(&hif, content(2)).unwrap();
        let second = metadata_source_revision(&asset);

        assert_ne!(first, second);
    }

    #[test]
    fn reads_embedded_raw_rating_and_label_without_a_sidecar() {
        let directory = tempdir().unwrap();
        let raw = directory.path().join("photo.ARW");
        fs::write(&raw, minimal_raw_with_xmp(1, "red")).unwrap();

        let metadata = MetadataFacade::default()
            .read_metadata(&raw, AssetKind::Raw)
            .unwrap();

        assert_eq!(metadata.rating, Some(1));
        assert_eq!(metadata.color_label.as_deref(), Some("Red"));
    }

    #[test]
    fn first_raw_rating_edit_preserves_embedded_color_label_in_the_new_sidecar() {
        let directory = tempdir().unwrap();
        let raw = directory.path().join("photo.ARW");
        fs::write(&raw, minimal_raw_with_xmp(1, "red")).unwrap();

        MetadataFacade::default()
            .patch_metadata(
                &raw,
                AssetKind::Raw,
                &oxy_domain::MetadataPatch {
                    rating: Some(Some(2)),
                    ..oxy_domain::MetadataPatch::default()
                },
            )
            .unwrap();
        let metadata = MetadataFacade::default()
            .read_metadata(&raw, AssetKind::Raw)
            .unwrap();

        assert_eq!(metadata.rating, Some(2));
        assert_eq!(metadata.color_label.as_deref(), Some("Red"));
    }

    #[test]
    fn enriches_raw_summary_from_embedded_xmp_without_a_sidecar() {
        let directory = tempdir().unwrap();
        let raw = directory.path().join("photo.ARW");
        fs::write(&raw, minimal_raw_with_xmp(1, "red")).unwrap();
        let mut assets =
            oxy_fs::scan_directory(directory.path(), &oxy_domain::AssetQuery::default(), 0)
                .unwrap()
                .items;

        MetadataFacade::default()
            .enrich_summaries(&mut assets)
            .unwrap();

        assert_eq!(assets[0].rating, Some(1));
        assert_eq!(assets[0].color_label.as_deref(), Some("Red"));
    }

    #[test]
    #[ignore = "requires OXY_RAW_XMP_FIXTURE to point to a RAW with embedded rating/color"]
    fn reads_external_raw_embedded_xmp_without_a_sidecar() {
        let raw = std::env::var_os("OXY_RAW_XMP_FIXTURE")
            .map(PathBuf::from)
            .expect("OXY_RAW_XMP_FIXTURE is required");
        let metadata = MetadataFacade::default()
            .read_metadata(&raw, AssetKind::Raw)
            .unwrap();

        assert!(metadata.rating.is_some());
        assert!(metadata.color_label.is_some());

        let directory = tempdir().unwrap();
        let copy = directory.path().join("fixture.ARW");
        fs::copy(raw, &copy).unwrap();
        MetadataFacade::default()
            .patch_metadata(
                &copy,
                AssetKind::Raw,
                &oxy_domain::MetadataPatch {
                    rating: Some(Some(2)),
                    ..oxy_domain::MetadataPatch::default()
                },
            )
            .unwrap();
        let patched = MetadataFacade::default()
            .read_metadata(&copy, AssetKind::Raw)
            .unwrap();
        assert_eq!(patched.rating, Some(2));
        assert_eq!(patched.color_label, metadata.color_label);
    }

    #[test]
    fn hif_edits_create_a_sidecar_without_touching_the_asset() {
        let directory = tempdir().unwrap();
        let hif = directory.path().join("photo.HIF");
        fs::write(&hif, b"camera image bytes").unwrap();

        patch_metadata(
            &hif,
            AssetKind::Heif,
            &oxy_domain::MetadataPatch {
                rating: Some(Some(4)),
                color_label: Some(Some("Green".into())),
                ..oxy_domain::MetadataPatch::default()
            },
        )
        .unwrap();

        assert_eq!(fs::read(&hif).unwrap(), b"camera image bytes");
        let metadata = read_metadata_with_exiftool(
            &hif,
            AssetKind::Heif,
            Some(Path::new("/missing/exiftool")),
        )
        .unwrap();
        assert_eq!(metadata.rating, Some(4));
        assert_eq!(metadata.color_label.as_deref(), Some("Green"));
    }

    #[test]
    fn embedded_sync_requires_an_existing_sidecar() {
        let directory = tempdir().unwrap();
        let hif = directory.path().join("photo.HIF");
        fs::write(&hif, b"camera image bytes").unwrap();

        let result = MetadataFacade::default().sync_metadata_to_embedded(&hif);
        assert!(matches!(result, Err(MetadataError::SidecarUnavailable(_))));
    }

    #[test]
    fn parses_and_normalizes_hif_rating_and_color_label() {
        let metadata = metadata_from_json(&serde_json::json!({
            "SourceFile": "D:/photos/DSC04979.HIF",
            "Rating": 1,
            "Label": "red"
        }));

        assert_eq!(metadata.rating, Some(1));
        assert_eq!(metadata.color_label.as_deref(), Some("Red"));
    }

    #[test]
    fn treats_sony_hif_none_label_as_unlabeled() {
        let metadata = metadata_from_json(&serde_json::json!({
            "SourceFile": "D:/photos/DSC04979.HIF",
            "Rating": 1,
            "Label": "None"
        }));

        assert_eq!(metadata.rating, Some(1));
        assert_eq!(metadata.color_label, None);
    }

    #[test]
    fn reads_sony_hif_xmp_without_exiftool() {
        let directory = tempdir().unwrap();
        let hif = directory.path().join("photo.HIF");
        fs::write(
            &hif,
            br#"....ftypSHIF....application/rdf+xml....<x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF><rdf:Description xmp:Rating='3' xmp:Label='red'/></rdf:RDF></x:xmpmeta>"#,
        )
        .unwrap();

        let metadata = read_metadata_with_exiftool(
            &hif,
            AssetKind::Heif,
            Some(Path::new("/missing/exiftool")),
        )
        .unwrap();

        assert_eq!(metadata.rating, Some(3));
        assert_eq!(metadata.color_label.as_deref(), Some("Red"));
    }

    #[test]
    fn reads_repository_hif_embedded_rating_without_exiftool() {
        let hif = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        let metadata = read_metadata_with_exiftool(
            &hif,
            AssetKind::Heif,
            Some(Path::new("/missing/exiftool")),
        )
        .unwrap();

        assert_eq!(metadata.rating, Some(0));
    }

    #[test]
    #[ignore = "requires OXY_HIF_XMP_FIXTURE to point to a Sony HIF with embedded rating/color"]
    fn reads_external_hif_embedded_xmp_without_exiftool() {
        let hif = PathBuf::from(std::env::var_os("OXY_HIF_XMP_FIXTURE").unwrap());
        let metadata = read_metadata_with_exiftool(
            &hif,
            AssetKind::Heif,
            Some(Path::new("/missing/exiftool")),
        )
        .unwrap();

        assert!(metadata.rating.is_some() || metadata.color_label.is_some());
        eprintln!("{}: {metadata:?}", hif.display());
    }

    #[test]
    fn builds_sony_viewer_compatible_hif_patch_arguments() {
        let mut command = Command::new("exiftool");
        add_patch_args(
            &mut command,
            &oxy_domain::MetadataPatch {
                rating: Some(None),
                color_label: Some(Some("Red".into())),
                ..oxy_domain::MetadataPatch::default()
            },
            true,
        )
        .unwrap();
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(args, ["-XMP:Rating=0", "-XMP:Label=red"]);
    }

    #[test]
    fn rejects_unsupported_sony_hif_color_label() {
        let mut command = Command::new("exiftool");
        let result = add_patch_args(
            &mut command,
            &oxy_domain::MetadataPatch {
                color_label: Some(Some("Purple".into())),
                ..oxy_domain::MetadataPatch::default()
            },
            true,
        );

        assert!(matches!(
            result,
            Err(MetadataError::UnsupportedHifColorLabel(_))
        ));
    }

    #[test]
    fn rotates_focus_location_and_exact_frame_size() {
        let info = orient_focus_info(7_008, 4_672, 1_204, 2_277, Some((219, 217)), 6);
        assert_eq!(info.coordinate_width, 4_672);
        assert_eq!(info.coordinate_height, 7_008);
        assert_eq!(info.regions[0].center_x, 2_395);
        assert_eq!(info.regions[0].center_y, 1_204);
        assert_eq!(info.regions[0].width, Some(217));
        assert_eq!(info.regions[0].height, Some(219));
    }

    #[test]
    fn reads_and_orients_repository_sony_hif_focus_metadata() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        let (capture, focus) = read_capture_details(&path, Some((4_672, 7_008))).unwrap();
        let focus = focus.expect("repository HIF fixture has Sony FocusLocation");

        assert_eq!(focus.coordinate_width, 4_672);
        assert_eq!(focus.coordinate_height, 7_008);
        assert_eq!(focus.regions.len(), 1);
        assert_eq!(focus.regions[0].center_x, 2_327);
        assert_eq!(focus.regions[0].center_y, 1_489);
        assert_eq!(focus.regions[0].width, Some(154));
        assert_eq!(focus.regions[0].height, Some(153));
        assert_eq!(capture.camera_make.as_deref(), Some("SONY"));
        assert!(
            capture
                .aperture
                .as_deref()
                .is_some_and(|value| value.starts_with("f/"))
        );
        assert!(
            capture
                .focal_length
                .as_deref()
                .is_some_and(|value| value.ends_with(" mm"))
        );
        assert!(capture.captured_at.is_some());
    }

    #[test]
    #[ignore = "requires OXY_FOCUS_FIXTURE to point to a Sony image with FocusLocation metadata"]
    fn reads_sony_focus_fixture_from_any_supported_container() {
        let path = std::env::var("OXY_FOCUS_FIXTURE").expect("set OXY_FOCUS_FIXTURE");
        let focus = read_focus_info(Path::new(&path), None)
            .expect("parse metadata")
            .expect("Sony FocusLocation");
        assert!(focus.coordinate_width > 0);
        assert!(focus.coordinate_height > 0);
        assert!(!focus.regions.is_empty());
        println!("{focus:?}");
    }

    #[test]
    fn creates_adobe_style_sidecar_with_escaped_values() {
        let directory = tempdir().unwrap();
        let raw = directory.path().join("photo.nef");
        fs::File::create(&raw).unwrap();
        let metadata = EditableMetadata {
            rating: Some(4),
            title: Some("Light & shadow".into()),
            keywords: vec!["travel".into(), "night".into()],
            ..EditableMetadata::default()
        };

        let sidecar = write_raw_sidecar(&raw, &metadata).unwrap();
        let xml = fs::read_to_string(sidecar).unwrap();
        assert!(xml.contains("xmp:Rating=\"4\""));
        assert!(xml.contains("Light &amp; shadow"));
        assert!(xml.contains("<rdf:li>travel</rdf:li>"));
    }

    #[test]
    fn patches_and_reads_raw_rating_and_label_without_losing_other_xmp() {
        let directory = tempdir().unwrap();
        let raw = directory.path().join("photo.arw");
        fs::File::create(&raw).unwrap();
        fs::write(
            sidecar_path(&raw),
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:custom="urn:test" xmp:Rating="2" xmp:Label="Red" custom:Keep="yes" /></rdf:RDF></x:xmpmeta>"#,
        )
        .unwrap();

        patch_sidecar(
            &raw,
            &oxy_domain::MetadataPatch {
                rating: Some(Some(5)),
                color_label: Some(Some("Blue & Cyan".into())),
                ..oxy_domain::MetadataPatch::default()
            },
        )
        .unwrap();

        let metadata = read_sidecar(&raw).unwrap();
        assert_eq!(metadata.rating, Some(5));
        assert_eq!(metadata.color_label.as_deref(), Some("Blue & Cyan"));
        let xml = fs::read_to_string(sidecar_path(&raw)).unwrap();
        assert!(xml.contains("custom:Keep=\"yes\""));
        assert!(xml.contains("xmp:Label=\"Blue &amp; Cyan\""));
        assert!(xml.contains("xmp:Label=\"Blue &amp; Cyan\" />"));
    }

    #[test]
    fn clearing_raw_fields_removes_their_attributes() {
        let directory = tempdir().unwrap();
        let raw = directory.path().join("photo.nef");
        fs::File::create(&raw).unwrap();
        write_raw_sidecar(
            &raw,
            &EditableMetadata {
                rating: Some(3),
                color_label: Some("Yellow".into()),
                ..EditableMetadata::default()
            },
        )
        .unwrap();

        patch_sidecar(
            &raw,
            &oxy_domain::MetadataPatch {
                rating: Some(None),
                color_label: Some(None),
                ..oxy_domain::MetadataPatch::default()
            },
        )
        .unwrap();

        let metadata = read_sidecar(&raw).unwrap();
        assert_eq!(metadata.rating, None);
        assert_eq!(metadata.color_label, None);
    }

    #[test]
    fn patches_element_style_raw_xmp_values() {
        let xml = r#"<rdf:Description xmlns:rdf="urn:rdf" xmlns:xmp="urn:xmp"><xmp:Rating>1</xmp:Rating><xmp:Label>Red</xmp:Label></rdf:Description>"#;
        let xml = set_xmp_attribute(xml, "Rating", Some("4")).unwrap();
        let xml = set_xmp_attribute(&xml, "Label", None).unwrap();

        assert!(xml.contains("<xmp:Rating>4</xmp:Rating>"));
        assert!(!xml.contains("xmp:Label"));
    }
}
