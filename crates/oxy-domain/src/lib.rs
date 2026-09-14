use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

mod raw;
pub use raw::*;
mod external_apps;
pub use external_apps::*;

pub type AssetId = String;
pub type JobId = String;
pub type SessionId = String;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileDeletionMode {
    #[default]
    Trash,
    Permanent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderSession {
    pub id: SessionId,
    pub root_path: PathBuf,
    pub display_name: String,
    pub opened_at_ms: u64,
    #[serde(default)]
    pub deletion_mode: FileDeletionMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySummary {
    pub path: PathBuf,
    pub name: String,
    pub has_children: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryTreeNode {
    pub entry: DirectorySummary,
    pub expanded: bool,
    /// `None` means this level has not been loaded yet. An empty vector is an
    /// authoritative leaf result.
    pub children: Option<Vec<DirectoryTreeNode>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryTreeSnapshot {
    pub session_id: SessionId,
    pub revision: u64,
    pub root: DirectoryTreeNode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySearchMatch {
    pub directory: DirectorySummary,
    pub ancestors: Vec<DirectorySummary>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Raw,
    Jpeg,
    Heif,
    Png,
    Tiff,
    Webp,
}

impl AssetKind {
    /// Classifies a supported asset path by extension without accessing the
    /// filesystem.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(std::ffi::OsStr::to_str)
            .and_then(Self::from_extension)
    }

    /// Classifies a supported asset extension case-insensitively.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "arw" | "cr2" | "cr3" | "nef" | "dng" | "raf" | "rw2" | "orf" => Some(Self::Raw),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "heif" | "heic" | "hif" => Some(Self::Heif),
            "png" => Some(Self::Png),
            "tif" | "tiff" => Some(Self::Tiff),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }
}

/// A portable review flag stored in the digiKam XMP namespace. The numeric
/// wire values are handled by the metadata crate; serialized IPC uses names.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PickLabel {
    Rejected,
    Pending,
    Accepted,
}

impl PickLabel {
    /// Canonical lowercase name. Serialization, the library projection cache,
    /// and query filters all share this spelling so a filter value never
    /// drifts from what the UI displays.
    pub fn as_str(self) -> &'static str {
        match self {
            PickLabel::Rejected => "rejected",
            PickLabel::Pending => "pending",
            PickLabel::Accepted => "accepted",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetSummary {
    pub id: AssetId,
    pub path: PathBuf,
    pub name: String,
    pub extension: String,
    pub kind: AssetKind,
    pub size_bytes: u64,
    pub modified_at_ms: u64,
    pub has_sidecar: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick_label: Option<PickLabel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditableMetadata {
    pub rating: Option<u8>,
    pub color_label: Option<String>,
    pub pick_label: Option<PickLabel>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
    pub keywords: Vec<String>,
    #[serde(default)]
    pub hierarchical_keywords: Vec<String>,
}

pub type CustomTagId = i64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CustomTag {
    pub id: CustomTagId,
    pub parent_id: Option<CustomTagId>,
    pub name: String,
    pub path: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetTagAssignment {
    pub tag: CustomTag,
    pub assigned_count: usize,
    pub asset_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetTagAssignmentsByPath {
    pub path: PathBuf,
    pub assignments: Vec<AssetTagAssignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TagDeleteImpact {
    pub tag_count: usize,
    pub asset_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TagSyncStatus {
    pub pending_count: usize,
    pub failed_count: usize,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MetadataProvider {
    Sidecar,
    Native,
    Exiftool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataCapability {
    pub provider: MetadataProvider,
    pub readable: bool,
    pub writable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FocusRegion {
    /// Center point in the camera focus-coordinate space.
    pub center_x: u32,
    pub center_y: u32,
    /// Exact focus-frame size when the camera provides it. Older Sony files
    /// expose only the focus location, so these fields are optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FocusInfo {
    /// Dimensions of the coordinate space stored by the camera. They can
    /// differ from an embedded RAW preview's dimensions and aspect ratio.
    pub coordinate_width: u32,
    pub coordinate_height: u32,
    pub regions: Vec<FocusRegion>,
}

/// Immutable camera and capture values read from the image's EXIF payload.
/// Values are kept display-ready because EXIF permits multiple underlying
/// representations for the same field (for example ISO and exposure time).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct CaptureMetadata {
    pub aperture: Option<String>,
    pub exposure_time: Option<String>,
    pub focal_length: Option<String>,
    pub iso: Option<String>,
    pub exposure_compensation: Option<String>,
    pub captured_at: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens_make: Option<String>,
    pub lens_model: Option<String>,
    pub chroma_subsampling: Option<String>,
    pub color_temperature: Option<String>,
    pub tint: Option<String>,
    pub dynamic_range_optimizer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetDetails {
    pub asset: AssetSummary,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub metadata: EditableMetadata,
    #[serde(default)]
    pub embedded_keywords: Vec<String>,
    #[serde(default)]
    pub embedded_hierarchical_keywords: Vec<String>,
    pub metadata_capability: MetadataCapability,
    pub sidecar_path: Option<PathBuf>,
    #[serde(default)]
    pub capture_metadata: CaptureMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_info: Option<FocusInfo>,
}

/// Rust-owned, versioned metadata read model shared by every UI surface.
/// A missing rating or color label is an authoritative empty value when
/// `status` is `Ready`, rather than a field that a caller may freely replace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataProjection {
    pub path: PathBuf,
    pub source_revision: String,
    /// Monotonic sequence assigned whenever Rust accepts a new state snapshot.
    pub state_revision: u64,
    pub valid_at: u64,
    pub status: ResourceLoadStatus,
    pub rating: Option<u8>,
    pub color_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick_label: Option<PickLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetDetailsResult {
    pub details: AssetDetails,
    pub metadata_projection: MetadataProjection,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ResourceLoadStatus {
    Loading,
    Ready,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum MetadataRequestPriority {
    Background,
    Filter,
    Visible,
    Selected,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "camelCase")]
pub enum RenderLevel {
    /// Fastest representation used by grids, lists, and filmstrips.
    Thumbnail,
    /// Stable fit-to-window representation shown when entering loupe.
    Preview,
    /// Best representation available for pixel inspection.
    Full,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum PreviewPriority {
    Preload,
    Nearby,
    Visible,
    Loupe,
}

/// Read-only diagnostics for one queued or running unit of work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DebugQueueItem {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_path: Option<PathBuf>,
    pub stage: String,
    pub priority: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<i64>,
    pub consumers: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_count: Option<usize>,
}

/// A point-in-time view of a scheduler. This contract is intentionally
/// read-only so diagnostics cannot alter production scheduling.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DebugQueueState {
    pub name: String,
    pub concurrency: usize,
    pub pending: Vec<DebugQueueItem>,
    pub active: Vec<DebugQueueItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DebugQueueSnapshot {
    pub captured_at_unix_ms: u64,
    pub worker_wait_micros: u64,
    pub collection_micros: u64,
    pub stale_queues: Vec<String>,
    pub queues: Vec<DebugQueueState>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SchedulePlacement {
    Front,
    Back,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OmittedScheduleAction {
    Release,
    Demote,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewScheduleIntent {
    pub path: PathBuf,
    pub level: RenderLevel,
    pub priority: PreviewPriority,
    pub rank: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOmittedPolicy {
    pub action: OmittedScheduleAction,
    pub priority: Option<PreviewPriority>,
    pub placement: Option<SchedulePlacement>,
}

/// Rust-owned state for one semantic render level of an asset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageProjection {
    pub path: PathBuf,
    pub source_revision: String,
    /// Monotonic sequence assigned whenever Rust accepts a new state snapshot.
    pub state_revision: u64,
    pub valid_at: u64,
    pub status: ResourceLoadStatus,
    pub level: RenderLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<PreviewResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PreviewKind {
    Embedded,
    Developed,
    Decoded,
    System,
    Original,
}

/// Format-agnostic decode diagnostics. Generalizes `HeifDiagnostics` so any
/// decoder (RAW, system, future formats) can report timing/backend info.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PreviewDiagnostics {
    /// Bytes returned by source-file reads, including bounded header read-ahead.
    /// This is not physical disk or network traffic (OS caches may satisfy it).
    pub source_read_bytes: Option<u64>,
    pub source_read_calls: Option<u64>,
    pub probe_ms: Option<u64>,
    /// Human-readable backend label, e.g. `"libraw-embedded"` or
    /// `"ffmpeg-hevc-tile-grid"`.
    pub backend: Option<String>,
    /// Time spent waiting for the shared decode gate.
    pub queue_wait_ms: Option<u64>,
    /// Time spent waiting for another request for the same source file.
    pub source_wait_ms: Option<u64>,
    /// Decode wall time in milliseconds.
    pub decode_ms: Option<u64>,
    /// Cache image encoding/write wall time in milliseconds.
    pub encode_ms: Option<u64>,
    /// Time spent durably syncing the temporary cache file.
    pub cache_sync_ms: Option<u64>,
    /// Time spent atomically committing the temporary cache file.
    pub cache_commit_ms: Option<u64>,
    /// Total measured backend wall time for this request.
    pub total_ms: Option<u64>,
    /// Why a fallback path was taken, if any.
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaSatisfaction {
    Satisfied,
    Interim,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaPersistence {
    NotApplicable,
    Pending,
    Persisted,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MediaResourceDescriptor {
    pub resource_id: String,
    pub url: String,
    pub media_type: String,
}

/// A neutral extent in image pixels, independent of orientation and ownership.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct PixelDimensions {
    pub width: u32,
    pub height: u32,
}

impl From<(u32, u32)> for PixelDimensions {
    fn from((width, height): (u32, u32)) -> Self {
        Self { width, height }
    }
}

/// Pixel storage before EXIF orientation is applied.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct EncodedDimensions(pub PixelDimensions);

/// Image pixels in viewing orientation; never CSS/layout units.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct DisplayDimensions(pub PixelDimensions);

impl EncodedDimensions {
    pub fn to_display(self, exif_orientation: u8) -> DisplayDimensions {
        let mut dimensions = self.0;
        if (5..=8).contains(&exif_orientation) {
            std::mem::swap(&mut dimensions.width, &mut dimensions.height);
        }
        DisplayDimensions(dimensions)
    }
}

/// Content provenance, independent of decoder, cache location, or render level.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ImageOrigin {
    PrimaryImage,
    EmbeddedPreview,
    RawSensor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct SourceImage {
    pub exif_orientation: u8,
    pub revision_id: String,
    /// Identifies the selected container image, never its current cache path.
    pub candidate_id: String,
    pub origin: ImageOrigin,
    pub encoded_dimensions: Option<EncodedDimensions>,
    pub display_dimensions: DisplayDimensions,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Sampling {
    Native,
    Reduced,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct DetailCoverage {
    pub reference_dimensions: DisplayDimensions,
    pub region: PreviewContentRect,
    /// Finest remaining sample grid; later upscaling cannot increase it.
    pub sampled_dimensions: DisplayDimensions,
    pub sampling: Sampling,
}

impl DetailCoverage {
    pub fn covers_reference(&self) -> bool {
        let size = self.reference_dimensions.0;
        self.region.x == 0
            && self.region.y == 0
            && self.region.width == size.width
            && self.region.height == size.height
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ByteIntegrity {
    SourceFile,
    SourcePayload,
    MetadataAdjusted,
    Reencoded,
    /// A backend did not provide proof of byte preservation.
    Unverified,
}

/// Completed operations only. Planned operations must never populate this list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ImageOperation {
    Decode {
        backend: String,
    },
    Develop {
        backend: String,
    },
    Resize {
        from: DisplayDimensions,
        to: DisplayDimensions,
    },
    Orient {
        exif: u8,
    },
    Encode {
        format: String,
    },
    ColorConvert {
        target: String,
    },
    Sharpen,
    Crop {
        region: PreviewContentRect,
    },
    Pad {
        canvas: DisplayDimensions,
    },
    MetadataEdit,
    Assemble {
        mode: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactFacts {
    pub exif_orientation: u8,
    pub source: SourceImage,
    pub encoded_dimensions: EncodedDimensions,
    pub display_dimensions: DisplayDimensions,
    pub detail: DetailCoverage,
    pub processing: Vec<ImageOperation>,
    pub byte_integrity: ByteIntegrity,
}

impl ArtifactFacts {
    /// Checks coordinate frames and coverage independently of cache/storage policy.
    pub fn is_consistent(&self) -> bool {
        let nonzero = |size: PixelDimensions| size.width > 0 && size.height > 0;
        let reference = self.detail.reference_dimensions.0;
        let region = self.detail.region;
        (1..=8).contains(&self.exif_orientation)
            && (1..=8).contains(&self.source.exif_orientation)
            && !self.source.candidate_id.is_empty()
            && nonzero(self.encoded_dimensions.0)
            && nonzero(self.source.display_dimensions.0)
            && nonzero(reference)
            && nonzero(self.detail.sampled_dimensions.0)
            && self.encoded_dimensions.to_display(self.exif_orientation) == self.display_dimensions
            && self.source.encoded_dimensions.is_none_or(|encoded| {
                encoded.to_display(self.source.exif_orientation) == self.source.display_dimensions
            })
            && region.width > 0
            && region.height > 0
            && region
                .x
                .checked_add(region.width)
                .is_some_and(|end| end <= reference.width)
            && region
                .y
                .checked_add(region.height)
                .is_some_and(|end| end <= reference.height)
    }

    pub fn native_detail(&self) -> bool {
        self.source.origin != ImageOrigin::EmbeddedPreview
            && self.detail.sampling == Sampling::Native
            && self.detail.covers_reference()
            && self.detail.sampled_dimensions.0.width >= self.detail.region.width
            && self.detail.sampled_dimensions.0.height >= self.detail.region.height
    }

    /// Derivation preserves original content identity and can only lose sampling detail.
    pub fn resize(&mut self, output: DisplayDimensions) {
        let from = self.display_dimensions;
        if from != output {
            if output.0.width < from.0.width || output.0.height < from.0.height {
                self.detail.sampling = Sampling::Reduced;
            }
            self.processing
                .push(ImageOperation::Resize { from, to: output });
        }
        let sampled = &mut self.detail.sampled_dimensions.0;
        sampled.width = sampled.width.min(output.0.width);
        sampled.height = sampled.height.min(output.0.height);
        self.display_dimensions = output;
    }

    /// Retain the legacy UI label only; no quality or cache decision uses it.
    pub fn preview_kind(&self) -> PreviewKind {
        match self.source.origin {
            ImageOrigin::EmbeddedPreview => PreviewKind::Embedded,
            ImageOrigin::RawSensor => PreviewKind::Developed,
            ImageOrigin::PrimaryImage if self.byte_integrity == ByteIntegrity::SourceFile => {
                PreviewKind::Original
            }
            ImageOrigin::PrimaryImage if self.processing.iter().any(|step| matches!(step, ImageOperation::Decode { backend } if backend.starts_with("system:"))) => PreviewKind::System,
            ImageOrigin::PrimaryImage => PreviewKind::Decoded,
        }
    }
}

/// Geometry in display-oriented pixels. The content rectangle covers the whole
/// logical display canvas; encoded padding lies outside that rectangle.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct PreviewGeometry {
    pub display_size: PreviewDisplaySize,
    pub content_rect: PreviewContentRect,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct PreviewDisplaySize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct PreviewContentRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResult {
    /// Actual source, geometry, and completed transformations; retained through cache recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_facts: Option<ArtifactFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<PreviewGeometry>,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub kind: PreviewKind,
    /// Semantic level this result fulfills. Pixel dimensions deliberately do
    /// not define the level: a format/platform policy may use one artifact for
    /// multiple levels (for example Sony HIF's 160 px JPEG for thumbnail and
    /// preview).
    pub render_level: RenderLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<MediaResourceDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub satisfaction: Option<MediaSatisfaction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence: Option<MediaPersistence>,
    /// Optional decode diagnostics. Populated by decoders that measure
    /// backend/timing; absent for cache hits and direct passthrough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<PreviewDiagnostics>,
}

/// User-visible policy and current state for the rebuildable preview cache.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CacheSettings {
    /// The app-owned directory that contains preview artifacts.
    pub location: PathBuf,
    pub default_location: PathBuf,
    pub custom_parent: Option<PathBuf>,
    pub is_custom_location: bool,
    pub max_size_bytes: u64,
    pub used_size_bytes: u64,
}

/// Mutable cache preferences. `custom_parent = None` restores the platform
/// default; custom parents always receive an app-owned child directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CacheSettingsUpdate {
    pub custom_parent: Option<PathBuf>,
    pub max_size_bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HeifBackendKind {
    CachedArtifact,
    WindowsWic,
    AppleImageIo,
    LinuxVaapi,
    FfmpegSoftware,
    LibheifSoftware,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AccelerationKind {
    Hardware,
    Software,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HeifDecodeStatus {
    Decoding,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HeifCapabilities {
    pub backend: HeifBackendKind,
    pub acceleration: AccelerationKind,
    pub available: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HeifDecodeSession {
    pub id: SessionId,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub tile_size: u32,
    pub expected_tiles: u32,
    pub backend: HeifBackendKind,
    pub acceleration: AccelerationKind,
    pub status: HeifDecodeStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "delivery", rename_all = "camelCase")]
pub enum HeifFullPresentation {
    Artifact { projection: Box<ImageProjection> },
    Tiles { session: HeifDecodeSession },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HeifTileReady {
    pub session_id: SessionId,
    pub generation: u64,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub payload: HeifTilePayload,
    pub url: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HeifTilePayload {
    Rgba,
    Jpeg,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HeifDiagnostics {
    pub backend: HeifBackendKind,
    pub acceleration: AccelerationKind,
    pub codec: Option<String>,
    /// Time spent waiting for the shared foreground decode gate.
    pub queue_wait_ms: u64,
    /// Time spent inside the selected platform/codec decoder.
    pub decode_ms: u64,
    pub tile_publish_ms: u64,
    pub total_ms: u64,
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HeifStatusEvent {
    pub session_id: SessionId,
    pub generation: u64,
    pub status: HeifDecodeStatus,
    pub diagnostics: Option<HeifDiagnostics>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssetQuery {
    pub search: Option<String>,
    pub kind: Option<AssetKind>,
    pub minimum_rating: Option<u8>,
    #[serde(default)]
    pub color_labels: Vec<String>,
    #[serde(default)]
    pub pick_labels: Vec<String>,
    pub sort: AssetSort,
    pub direction: SortDirection,
    pub page_size: Option<usize>,
}

impl AssetQuery {
    /// True when a filter cannot be answered from a cheap directory scan and
    /// needs sidecar/XMP enrichment first. Keep this the single source of truth:
    /// a caller that skips enrichment silently drops every matching asset.
    pub fn needs_metadata_enrichment(&self) -> bool {
        self.minimum_rating.is_some()
            || !self.color_labels.is_empty()
            || !self.pick_labels.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AssetSort {
    #[default]
    Name,
    Modified,
    Size,
    Kind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<usize>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryBrowseProgress {
    pub session_id: String,
    pub directory: PathBuf,
    pub stage: String,
    pub source: String,
    pub discovered_count: usize,
    pub elapsed_ms: u64,
    pub cache_ms: u64,
    pub resolve_ms: u64,
    pub enumeration_ms: u64,
    pub attributes_ms: u64,
    pub snapshot_serialize_ms: u64,
    pub snapshot_persist_ms: u64,
    pub sort_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowsePage {
    #[serde(flatten)]
    pub page: Page<AssetSummary>,
    pub progress: DirectoryBrowseProgress,
    pub snapshot_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryIndexUpdate {
    pub root_path: PathBuf,
    pub asset_count: usize,
    pub directory_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MetadataPatch {
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub rating: Option<Option<u8>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub color_label: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub pick_label: Option<Option<PickLabel>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub title: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub creator: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub copyright: Option<Option<String>>,
    pub keywords: Option<Vec<String>>,
    pub hierarchical_keywords: Option<Vec<String>>,
}

fn deserialize_nullable_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FileOperation {
    Rename {
        source: PathBuf,
        new_name: String,
    },
    Copy {
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
    },
    Move {
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
    },
    Trash {
        paths: Vec<PathBuf>,
    },
    DeletePermanently {
        paths: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOperationResult {
    pub affected_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum JobPriority {
    LibraryIndex,
    DirectoryBackground,
    SelectedMetadata,
    VisibleThumbnail,
    LoupePreview,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    pub job_id: JobId,
    pub completed: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
}

/// A snapshot of registry ownership; file bytes are never encoded memory.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRegistryStats {
    pub max_entries: usize,
    pub max_encoded_bytes: usize,
    pub max_materialized_responses: usize,
    pub max_materialized_bytes: usize,
    pub entries: usize,
    pub encoded_bytes: usize,
    pub peak_encoded_bytes: usize,
    pub peak_entries: usize,
    pub ui_leased: usize,
    pub read_leased: usize,
    pub released_or_expired: usize,
    pub staged_files: usize,
    pub staged_bytes: u64,
    pub materialized_responses: usize,
    pub materialized_bytes: usize,
}

/// One automated performance scenario injected into the packaged app through
/// the `OXY_PERF_SCENARIO` environment variable. Consumed by the frontend
/// performance harness; see `docs/PERF_E2E.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfScenario {
    #[serde(default)]
    pub expected_assets: Option<usize>,
    pub name: String,
    pub folder: PathBuf,
    #[serde(default)]
    pub select_name: Option<String>,
    #[serde(default)]
    pub enter_loupe: Option<bool>,
    #[serde(default)]
    pub scroll_to_end: bool,
    #[serde(default)]
    pub resource_stress: Option<String>,
    #[serde(default)]
    pub await_marks: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    pub report_path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_enrichment_is_required_only_for_metadata_filters() {
        assert!(!AssetQuery::default().needs_metadata_enrichment());

        // Cheap filters are answerable from the directory scan alone.
        assert!(
            !AssetQuery {
                search: Some("beach".into()),
                kind: Some(AssetKind::Raw),
                ..AssetQuery::default()
            }
            .needs_metadata_enrichment()
        );

        for filter in [
            AssetQuery {
                minimum_rating: Some(1),
                ..AssetQuery::default()
            },
            AssetQuery {
                color_labels: vec!["red".into()],
                ..AssetQuery::default()
            },
            AssetQuery {
                pick_labels: vec!["accepted".into()],
                ..AssetQuery::default()
            },
        ] {
            assert!(filter.needs_metadata_enrichment());
        }
    }

    #[test]
    fn asset_kind_classifies_supported_paths_case_insensitively() {
        assert_eq!(
            AssetKind::from_path(Path::new("photo.ARW")),
            Some(AssetKind::Raw)
        );
        assert_eq!(
            AssetKind::from_path(Path::new("photo.JPEG")),
            Some(AssetKind::Jpeg)
        );
        assert_eq!(
            AssetKind::from_path(Path::new("photo.HIF")),
            Some(AssetKind::Heif)
        );
        assert_eq!(
            AssetKind::from_path(Path::new("photo.png")),
            Some(AssetKind::Png)
        );
        assert_eq!(
            AssetKind::from_path(Path::new("photo.TIFF")),
            Some(AssetKind::Tiff)
        );
        assert_eq!(
            AssetKind::from_path(Path::new("photo.webp")),
            Some(AssetKind::Webp)
        );
        assert_eq!(AssetKind::from_path(Path::new("photo.txt")), None);
        assert_eq!(AssetKind::from_path(Path::new("photo")), None);
    }

    #[test]
    fn metadata_patch_distinguishes_missing_fields_from_explicit_nulls() {
        let missing: MetadataPatch = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(missing.rating, None);
        assert_eq!(missing.color_label, None);
        assert_eq!(missing.pick_label, None);

        let cleared: MetadataPatch = serde_json::from_value(serde_json::json!({
            "rating": null,
            "colorLabel": null,
            "pickLabel": null
        }))
        .unwrap();
        assert_eq!(cleared.rating, Some(None));
        assert_eq!(cleared.color_label, Some(None));
        assert_eq!(cleared.pick_label, Some(None));

        let assigned: MetadataPatch = serde_json::from_value(serde_json::json!({
            "rating": 4,
            "colorLabel": "Purple",
            "pickLabel": "accepted"
        }))
        .unwrap();
        assert_eq!(assigned.rating, Some(Some(4)));
        assert_eq!(assigned.color_label, Some(Some("Purple".into())));
        assert_eq!(assigned.pick_label, Some(Some(PickLabel::Accepted)));
    }
}
