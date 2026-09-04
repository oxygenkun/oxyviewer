use super::TagLookup;
use oxy_domain::CaptureMetadata;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImageType {
    Jpeg,
    Heif,
    Raw,
    Tiff,
    Other,
}

impl ImageType {
    pub(super) fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("jpg" | "jpeg") => Self::Jpeg,
            Some("heif" | "heic" | "hif" | "avif") => Self::Heif,
            Some("arw" | "cr2" | "cr3" | "nef" | "dng" | "raf" | "rw2" | "orf") => Self::Raw,
            Some("tif" | "tiff") => Self::Tiff,
            _ => Self::Other,
        }
    }
}

pub(super) fn apply(image_type: ImageType, values: &TagLookup<'_>, capture: &mut CaptureMetadata) {
    let value = match image_type {
        ImageType::Jpeg => values
            .value("JPEG", "ChromaSubsampling")
            .or_else(|| values.value("EXIF", "YCbCrSubSampling")),
        ImageType::Heif => values
            .value("HEIF", "ChromaFormat")
            .or_else(|| values.value("EXIF", "YCbCrSubSampling")),
        ImageType::Raw | ImageType::Tiff | ImageType::Other => {
            values.value("EXIF", "YCbCrSubSampling")
        }
    };
    capture.chroma_subsampling = value.and_then(normalize_chroma_subsampling);
}

fn normalize_chroma_subsampling(value: &str) -> Option<String> {
    let compact = value
        .trim()
        .trim_start_matches("YCbCr")
        .replace([',', 'x'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let normalized = match compact.as_str() {
        "4:0:0" | "4:1:0" | "4:1:1" | "4:2:0" | "4:2:2" | "4:4:0" | "4:4:4" => compact.as_str(),
        "1 1" => "4:4:4",
        "1 2" => "4:4:0",
        "2 1" => "4:2:2",
        "2 2" => "4:2:0",
        "4 1" => "4:1:1",
        "4 2" => "4:1:0",
        _ => return None,
    };
    Some(normalized.to_owned())
}
