//! Values owned by the media engine, independent of application state.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub mod raw;
pub use raw::*;
mod analysis_image;
pub use analysis_image::*;

#[cfg(test)]
mod tests;

pub type SessionId = String;

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

/// Display-ready camera and capture values. Corresponding XMP fields override
/// embedded EXIF/MakerNotes, with adjacent sidecar XMP taking precedence over
/// embedded XMP. Fields absent from XMP retain their camera value.
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
    /// Drive mode the shot was taken in, for example `Continuous`.
    pub release_mode: Option<String>,
    /// 1-based index of the frame inside its burst, when the camera recorded
    /// one. `None` means the frame is not part of a burst (single shot, or the
    /// camera did not report an index).
    pub sequence_number: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "camelCase")]
pub enum RenderLevel {
    /// Fastest representation used by grids and filmstrips.
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
