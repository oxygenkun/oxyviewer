use crate::{MetadataError, normalize_color_label, xmp_value};
use libheif_rs::HeifContext;
use oxy_domain::{CaptureMetadata, EditableMetadata, FocusInfo};
use oxy_metadata_parser::{Tag, core::TagValue};
use std::{fs, io::Read, path::Path};

/// A format-neutral metadata tag retained alongside OxyViewer's normalized
/// fields. Unknown tags remain available when the application data model does
/// not yet have a dedicated field for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMetadataTag {
    pub namespace: String,
    pub name: String,
    pub value: String,
}

/// The complete result of one metadata read, independent of the source file's
/// container and metadata standards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataDocument {
    pub editable: EditableMetadata,
    pub capture: CaptureMetadata,
    pub focus: Option<FocusInfo>,
    pub raw: Vec<RawMetadataTag>,
    pub diagnostics: Vec<String>,
}

/// Stable boundary implemented by native or compatibility readers.
pub trait MetadataReader: Send + Sync {
    fn read(
        &self,
        path: &Path,
        display_dimensions: Option<(u32, u32)>,
    ) -> Result<MetadataDocument, MetadataError>;
}

/// OxyViewer's in-process reader. The vendored parser handles the common
/// image/RAW containers, while libheif supplies authoritative BMFF metadata
/// items for HEIF/HIF/AVIF instead of relying on byte-pattern scans.
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeMetadataReader;

impl MetadataReader for NativeMetadataReader {
    fn read(
        &self,
        path: &Path,
        display_dimensions: Option<(u32, u32)>,
    ) -> Result<MetadataDocument, MetadataError> {
        let tags = oxy_metadata_parser::tags(path)
            .map_err(|error| MetadataError::Read(error.to_string()))?;
        let mut document = document_from_tags(&tags, display_dimensions);

        if is_heif_path(path) {
            match read_heif_xmp(path) {
                Ok(Some(xmp)) => {
                    overlay_xmp(&mut document.editable, &xmp);
                    append_xmp_raw_tags(&mut document.raw, &xmp);
                }
                Ok(None) => {}
                Err(error) => {
                    document.diagnostics.push(error.to_string());
                    if let Some(xmp) = read_bounded_xmp_fallback(path)? {
                        overlay_xmp(&mut document.editable, &xmp);
                        append_xmp_raw_tags(&mut document.raw, &xmp);
                    }
                }
            }
        }

        Ok(document)
    }
}

fn document_from_tags(tags: &[Tag], display_dimensions: Option<(u32, u32)>) -> MetadataDocument {
    let value = |group: &str, name: &str| {
        tags.iter()
            .find(|tag| tag.group == group && tag.name == name)
            .map(|tag| tag.value.as_str())
    };

    let editable = EditableMetadata {
        rating: value("XMP", "Rating").and_then(|value| value.parse().ok()),
        color_label: value("XMP", "Label")
            .map(str::to_owned)
            .and_then(normalize_color_label),
        title: value("XMP", "Title").map(str::to_owned),
        description: value("XMP", "Description").map(str::to_owned),
        creator: value("XMP", "Creator").map(str::to_owned),
        copyright: value("XMP", "Rights")
            .or_else(|| value("XMP", "Copyright"))
            .map(str::to_owned),
        keywords: value("XMP", "Subject").map(split_list).unwrap_or_default(),
    };

    let capture = CaptureMetadata {
        aperture: value("EXIF", "FNumber")
            .or_else(|| value("Composite", "Aperture"))
            .map(|value| format!("f/{}", trim_fraction(value))),
        exposure_time: value("EXIF", "ExposureTime")
            .or_else(|| value("Composite", "ShutterSpeed"))
            .map(|value| format!("{} s", trim_seconds(value))),
        focal_length: value("EXIF", "FocalLength").map(normalize_focal_length),
        iso: value("EXIF", "ISO").map(str::to_owned),
        exposure_compensation: value("EXIF", "ExposureCompensation")
            .map(normalize_exposure_compensation),
        captured_at: value("EXIF", "DateTimeOriginal").map(str::to_owned),
        camera_make: value("EXIF", "Make").map(str::to_owned),
        camera_model: value("EXIF", "Model").map(str::to_owned),
        lens_make: value("EXIF", "LensMake").map(str::to_owned),
        lens_model: value("EXIF", "LensModel").map(str::to_owned),
        chroma_subsampling: value("EXIF", "YCbCrSubSampling")
            .or_else(|| value("HEIF", "ChromaFormat"))
            .map(str::to_owned),
    };

    MetadataDocument {
        editable,
        capture,
        focus: focus_from_tags(tags, display_dimensions),
        raw: tags
            .iter()
            .map(|tag| RawMetadataTag {
                namespace: tag.group.to_owned(),
                name: tag.name.clone(),
                value: tag.value.clone(),
            })
            .collect(),
        diagnostics: Vec::new(),
    }
}

fn focus_from_tags(tags: &[Tag], display_dimensions: Option<(u32, u32)>) -> Option<FocusInfo> {
    let location = tags
        .iter()
        .find(|tag| tag.group == "MakerNotes" && tag.name == "FocusLocation")?;
    let values = unsigned_values(location);
    let [width, height, center_x, center_y, ..] = values.as_slice() else {
        return None;
    };
    if *width == 0 || *height == 0 || center_x > width || center_y > height {
        return None;
    }

    let frame_size = tags
        .iter()
        .find(|tag| tag.group == "MakerNotes" && tag.name == "FocusFrameSize")
        .and_then(parse_frame_size);
    let orientation = tags
        .iter()
        .find(|tag| tag.group == "EXIF" && tag.name == "Orientation")
        .and_then(|tag| tag.typed_value.as_ref())
        .and_then(TagValue::to_u32)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or_else(|| {
            display_dimensions
                .filter(|(display_width, display_height)| {
                    (*width > *height) != (*display_width > *display_height)
                })
                .map(|_| 6)
                .unwrap_or(1)
        });

    Some(crate::orient_focus_info(
        *width,
        *height,
        *center_x,
        *center_y,
        frame_size,
        orientation,
    ))
}

fn unsigned_values(tag: &Tag) -> Vec<u32> {
    match tag.typed_value.as_ref() {
        Some(TagValue::U16Array(values)) => values.iter().map(|value| u32::from(*value)).collect(),
        Some(TagValue::U32Array(values)) => values.clone(),
        _ => tag
            .value
            .split(|character: char| !character.is_ascii_digit())
            .filter(|value| !value.is_empty())
            .filter_map(|value| value.parse().ok())
            .collect(),
    }
}

fn parse_frame_size(tag: &Tag) -> Option<(u32, u32)> {
    let values = unsigned_values(tag);
    match values.as_slice() {
        [width, height, valid, ..] if *width > 0 && *height > 0 && *valid != 0 => {
            Some((*width, *height))
        }
        [width, height] if *width > 0 && *height > 0 => Some((*width, *height)),
        _ => None,
    }
}

fn read_heif_xmp(path: &Path) -> Result<Option<String>, MetadataError> {
    let context = HeifContext::read_from_file(&path.to_string_lossy())
        .map_err(|error| MetadataError::Read(error.to_string()))?;
    let handle = context
        .primary_image_handle()
        .map_err(|error| MetadataError::Read(error.to_string()))?;
    for item in handle.all_metadata() {
        if item.content_type == "application/rdf+xml" {
            return String::from_utf8(item.raw_data)
                .map(Some)
                .map_err(|error| MetadataError::Read(format!("invalid HEIF XMP: {error}")));
        }
    }
    Ok(None)
}

/// Compatibility path for damaged and synthetic BMFF files that libheif
/// refuses to open. Valid HEIF/HIF files always take the item-table path.
fn read_bounded_xmp_fallback(path: &Path) -> Result<Option<String>, MetadataError> {
    const LIMIT: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(LIMIT).read_to_end(&mut bytes)?;
    let start = bytes
        .windows(b"<x:xmpmeta".len())
        .position(|window| window == b"<x:xmpmeta");
    let Some(start) = start else {
        return Ok(None);
    };
    let close = b"</x:xmpmeta>";
    let Some(relative_end) = bytes[start..]
        .windows(close.len())
        .position(|window| window == close)
    else {
        return Ok(None);
    };
    let end = start + relative_end + close.len();
    std::str::from_utf8(&bytes[start..end])
        .map(|xml| Some(xml.to_owned()))
        .map_err(|error| MetadataError::Read(format!("invalid embedded XMP: {error}")))
}

fn overlay_xmp(metadata: &mut EditableMetadata, xml: &str) {
    metadata.rating = xmp_value(xml, "Rating")
        .and_then(|value| value.parse().ok())
        .or(metadata.rating);
    metadata.color_label = xmp_value(xml, "Label")
        .filter(|value| !value.is_empty())
        .and_then(normalize_color_label)
        .or_else(|| metadata.color_label.clone());
}

fn append_xmp_raw_tags(tags: &mut Vec<RawMetadataTag>, xml: &str) {
    for name in ["Rating", "Label"] {
        if let Some(value) = xmp_value(xml, name) {
            if let Some(existing) = tags
                .iter_mut()
                .find(|tag| tag.namespace == "XMP" && tag.name == name)
            {
                existing.value = value;
            } else {
                tags.push(RawMetadataTag {
                    namespace: "XMP".to_owned(),
                    name: name.to_owned(),
                    value,
                });
            }
        }
    }
}

fn is_heif_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["heif", "heic", "hif", "avif"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn trim_fraction(value: &str) -> String {
    let Some((numerator, denominator)) = value.split_once('/') else {
        return value.to_owned();
    };
    match (numerator.parse::<f64>(), denominator.parse::<f64>()) {
        (Ok(numerator), Ok(denominator)) if denominator != 0.0 => {
            crate::format_number(numerator / denominator)
        }
        _ => value.to_owned(),
    }
}

fn trim_seconds(value: &str) -> &str {
    value.trim().strip_suffix(" s").unwrap_or(value.trim())
}

fn normalize_focal_length(value: &str) -> String {
    let value = value.trim().strip_suffix(" mm").unwrap_or(value.trim());
    format!("{} mm", trim_fraction(value))
}

fn normalize_exposure_compensation(value: &str) -> String {
    let number = trim_fraction(value);
    format!(
        "{}{} EV",
        if number.starts_with('-') || number == "0" {
            ""
        } else {
            "+"
        },
        number
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_a_format_neutral_tag_document() {
        let tags = vec![
            Tag::new("EXIF", "Make", "SONY"),
            Tag::new("EXIF", "FNumber", "28/10"),
            Tag::new("XMP", "Rating", "4"),
            Tag::new("XMP", "Label", "red"),
        ];
        let document = document_from_tags(&tags, None);
        assert_eq!(document.capture.camera_make.as_deref(), Some("SONY"));
        assert_eq!(document.capture.aperture.as_deref(), Some("f/2.8"));
        assert_eq!(document.editable.rating, Some(4));
        assert_eq!(document.editable.color_label.as_deref(), Some("Red"));
        assert_eq!(document.raw.len(), 4);
    }
}
