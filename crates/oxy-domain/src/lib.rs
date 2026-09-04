use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type AssetId = String;
pub type JobId = String;
pub type SessionId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderSession {
    pub id: SessionId,
    pub root_path: PathBuf,
    pub display_name: String,
    pub opened_at_ms: u64,
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
pub struct DirectorySearchMatch {
    pub directory: DirectorySummary,
    pub ancestors: Vec<DirectorySummary>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Raw,
    Jpeg,
    Heif,
    Png,
    Tiff,
    Webp,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditableMetadata {
    pub rating: Option<u8>,
    pub color_label: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
    pub keywords: Vec<String>,
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
#[serde(rename_all = "camelCase")]
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetDetails {
    pub asset: AssetSummary,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub metadata: EditableMetadata,
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
    pub projection_revision: u64,
    pub valid_at: u64,
    pub status: ResourceLoadStatus,
    pub rating: Option<u8>,
    pub color_label: Option<String>,
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

/// Rust-owned state for one semantic render level of an asset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageProjection {
    pub path: PathBuf,
    pub source_revision: String,
    pub projection_revision: u64,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResult {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub kind: PreviewKind,
    /// Semantic level this result fulfills. Pixel dimensions deliberately do
    /// not define the level: a format/platform policy may use one artifact for
    /// multiple levels (for example Sony HIF's 160 px JPEG for thumbnail and
    /// preview on Windows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_level: Option<RenderLevel>,
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
    WindowsWic,
    WindowsMediaFoundation,
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
    Probing,
    Decoding,
    CompatibilityFallback,
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
pub struct HeifDecodeRequest {
    pub path: PathBuf,
    pub generation: u64,
    pub hardware_acceleration: bool,
    pub tile_size: u32,
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
#[serde(rename_all = "camelCase")]
pub struct HeifTileReady {
    pub session_id: SessionId,
    pub generation: u64,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    pub url: String,
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
    pub sort: AssetSort,
    pub direction: SortDirection,
    pub page_size: Option<usize>,
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
    pub rating: Option<Option<u8>>,
    pub color_label: Option<Option<String>>,
    pub title: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub creator: Option<Option<String>>,
    pub copyright: Option<Option<String>>,
    pub keywords: Option<Vec<String>>,
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

/// One automated performance scenario injected into the packaged app through
/// the `OXY_PERF_SCENARIO` environment variable. Consumed by the frontend
/// performance harness; see `docs/PERF_E2E.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfScenario {
    pub name: String,
    pub folder: PathBuf,
    #[serde(default)]
    pub select_name: Option<String>,
    #[serde(default)]
    pub enter_loupe: Option<bool>,
    #[serde(default)]
    pub await_marks: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    pub report_path: PathBuf,
}
