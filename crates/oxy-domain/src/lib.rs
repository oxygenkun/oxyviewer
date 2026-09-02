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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetDetails {
    pub asset: AssetSummary,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub metadata: EditableMetadata,
    pub sidecar_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_info: Option<FocusInfo>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PreviewMode {
    Thumbnail,
    LoupePreview,
    FullDetail,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PreviewPriority {
    Nearby,
    Visible,
    Loupe,
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

/// Semantic preview stage shared across formats. Replaces the magic numbers
/// `512` / `4_096` and lets the frontend/backend agree on which band of the
/// progressive pipeline a request belongs to.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PreviewStage {
    /// Cheap thumbnail (~512 px) used for grid/list/filmstrip.
    Thumb512,
    /// Loupe preview (~4096 px) shown while full detail develops.
    Loupe4096,
    /// Full-resolution output. For HEIF this is served by the tile session;
    /// for other formats it is a single full-resolution JPEG.
    Full,
}

impl PreviewStage {
    pub fn target_size(self) -> u32 {
        match self {
            PreviewStage::Thumb512 => 512,
            PreviewStage::Loupe4096 => 4_096,
            // 0 signals "no downscale" to the cache key / decoder.
            PreviewStage::Full => 0,
        }
    }
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
    /// Stage that produced this result, when known. `Original` results (direct
    /// raster passthrough) leave this as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<PreviewStage>,
    /// Optional decode diagnostics. Populated by decoders that measure
    /// backend/timing; absent for cache hits and direct passthrough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<PreviewDiagnostics>,
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
