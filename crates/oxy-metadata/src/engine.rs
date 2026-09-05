use crate::{
    MetadataError, is_rejected_xmp_rating, normalize_color_label, parse_pick_label,
    parse_xmp_rating, xmp_value,
};
use libheif_rs::{Chroma, ColorSpace, HeifContext};
use oxy_domain::{CaptureMetadata, EditableMetadata, FocusInfo};
use oxy_metadata_parser::{Tag, core::TagValue};
use sha2::{Digest, Sha256};
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
    pub embedded_editable: EditableMetadata,
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
        let mut document = document_from_tags(&tags, path, display_dimensions);

        if crate::capture::is_heif_path(path) {
            match read_heif_container_metadata(path) {
                Ok((xmp, chroma_subsampling)) => {
                    if let Some(chroma_subsampling) = chroma_subsampling {
                        document.capture.chroma_subsampling = Some(chroma_subsampling);
                    }
                    if let Some(xmp) = xmp {
                        overlay_xmp(&mut document.editable, &xmp);
                        append_xmp_raw_tags(&mut document.raw, &xmp);
                    }
                }
                Err(error) => {
                    document.diagnostics.push(error.to_string());
                    if let Some(xmp) = read_bounded_xmp_fallback(path)? {
                        overlay_xmp(&mut document.editable, &xmp);
                        append_xmp_raw_tags(&mut document.raw, &xmp);
                    }
                }
            }
        }

        document.embedded_editable = document.editable.clone();
        Ok(document)
    }
}

fn document_from_tags(
    tags: &[Tag],
    path: &Path,
    display_dimensions: Option<(u32, u32)>,
) -> MetadataDocument {
    let value = |group: &str, name: &str| {
        tags.iter()
            .find(|tag| tag.group == group && tag.name == name)
            .map(|tag| tag.value.as_str())
    };

    let rating_value = value("XMP", "Rating");
    let editable = EditableMetadata {
        rating: rating_value.and_then(parse_xmp_rating),
        color_label: value("XMP", "Label")
            .map(str::to_owned)
            .and_then(normalize_color_label),
        pick_label: value("XMP", "PickLabel")
            .and_then(parse_pick_label)
            .or_else(|| {
                rating_value
                    .filter(|value| is_rejected_xmp_rating(value))
                    .map(|_| oxy_domain::PickLabel::Rejected)
            }),
        title: value("XMP", "Title").map(str::to_owned),
        description: value("XMP", "Description").map(str::to_owned),
        creator: value("XMP", "Creator").map(str::to_owned),
        copyright: value("XMP", "Rights")
            .or_else(|| value("XMP", "Copyright"))
            .map(str::to_owned),
        keywords: value("XMP", "Subject").map(split_list).unwrap_or_default(),
        hierarchical_keywords: value("XMP", "HierarchicalSubject")
            .map(split_list)
            .unwrap_or_default(),
    };

    let capture = crate::capture::from_tags(tags, path);

    MetadataDocument {
        embedded_editable: editable.clone(),
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
        .or_else(|| {
            tags.iter()
                .find(|tag| tag.group == "HEIF" && tag.name == "Rotation")
                .and_then(|tag| tag.value.parse::<u16>().ok())
                .and_then(|rotation| match rotation {
                    0 => Some(1),
                    90 => Some(6),
                    180 => Some(3),
                    270 => Some(8),
                    _ => None,
                })
        })
        .unwrap_or_else(|| {
            display_dimensions
                .filter(|(display_width, display_height)| {
                    (*width > *height) != (*display_width > *display_height)
                })
                .map_or(1, |_| 6)
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
    read_heif_container_metadata(path).map(|(xmp, _)| xmp)
}

fn read_heif_container_metadata(
    path: &Path,
) -> Result<(Option<String>, Option<String>), MetadataError> {
    let context = HeifContext::read_from_file(&path.to_string_lossy())
        .map_err(|error| MetadataError::Read(error.to_string()))?;
    let handle = context
        .primary_image_handle()
        .map_err(|error| MetadataError::Read(error.to_string()))?;
    let chroma_subsampling = handle
        .preferred_decoding_colorspace()
        .ok()
        .and_then(|color_space| match color_space {
            ColorSpace::YCbCr(Chroma::C420) => Some("4:2:0".to_owned()),
            ColorSpace::YCbCr(Chroma::C422) => Some("4:2:2".to_owned()),
            ColorSpace::YCbCr(Chroma::C444) => Some("4:4:4".to_owned()),
            ColorSpace::Monochrome => Some("4:0:0".to_owned()),
            _ => None,
        });
    for item in handle.all_metadata() {
        if item.content_type == "application/rdf+xml" {
            return String::from_utf8(item.raw_data)
                .map(|xmp| (Some(xmp), chroma_subsampling))
                .map_err(|error| MetadataError::Read(format!("invalid HEIF XMP: {error}")));
        }
    }
    Ok((None, chroma_subsampling))
}

pub(crate) fn heif_metadata_digest(path: &Path) -> Option<u64> {
    let xmp = read_heif_xmp(path)
        .ok()
        .flatten()
        .or_else(|| read_bounded_xmp_fallback(path).ok().flatten())?;
    let digest = Sha256::digest(xmp.as_bytes());
    Some(u64::from_be_bytes(digest[..8].try_into().ok()?))
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
    let rating_value = xmp_value(xml, "Rating");
    if let Some(value) = rating_value.as_deref() {
        metadata.rating = parse_xmp_rating(value);
    }
    if let Some(value) = xmp_value(xml, "Label") {
        // An explicit Sony/Imaging Edge `None` is a clear operation, so it
        // must override a label discovered by the generic container parser.
        metadata.color_label = normalize_color_label(value);
    }
    metadata.pick_label = crate::xmp_prefixed_value(xml, "digiKam", "PickLabel")
        .as_deref()
        .and_then(parse_pick_label)
        .or_else(|| {
            rating_value
                .as_deref()
                .filter(|value| is_rejected_xmp_rating(value))
                .map(|_| oxy_domain::PickLabel::Rejected)
        })
        .or(metadata.pick_label);
    let keywords = crate::xmp_array_values(xml, "dc", "subject");
    if !keywords.is_empty() || xml.contains("<dc:subject") {
        metadata.keywords = keywords;
    }
    let hierarchical = crate::xmp_array_values(xml, "lr", "hierarchicalSubject");
    if !hierarchical.is_empty() || xml.contains("<lr:hierarchicalSubject") {
        metadata.hierarchical_keywords = hierarchical;
    }
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
    if let Some(value) = crate::xmp_prefixed_value(xml, "digiKam", "PickLabel") {
        if let Some(existing) = tags
            .iter_mut()
            .find(|tag| tag.namespace == "XMP" && tag.name == "PickLabel")
        {
            existing.value = value;
        } else {
            tags.push(RawMetadataTag {
                namespace: "XMP".to_owned(),
                name: "PickLabel".to_owned(),
                value,
            });
        }
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heif_counter_clockwise_rotation_orients_focus_coordinates() {
        let tags = [
            Tag::new("MakerNotes", "FocusLocation", "7008 4672 1204 2277"),
            Tag::new("MakerNotes", "FocusFrameSize", "219 217 1"),
            Tag::new("HEIF", "Rotation", "270"),
        ];

        let focus = focus_from_tags(&tags, Some((4_672, 7_008))).unwrap();
        assert_eq!(
            (focus.coordinate_width, focus.coordinate_height),
            (4_672, 7_008)
        );
        assert_eq!(focus.regions[0].center_x, 2_277);
        assert_eq!(focus.regions[0].center_y, 5_804);
        assert_eq!(focus.regions[0].width, Some(217));
        assert_eq!(focus.regions[0].height, Some(219));
    }

    #[test]
    fn normalizes_a_format_neutral_tag_document() {
        let tags = vec![
            Tag::new("EXIF", "Make", "SONY"),
            Tag::new("EXIF", "FNumber", "28/10"),
            Tag::new("XMP", "Rating", "4"),
            Tag::new("XMP", "Label", "red"),
            Tag::new("XMP", "PickLabel", "3"),
        ];
        let document = document_from_tags(&tags, Path::new("photo.jpg"), None);
        assert_eq!(document.capture.camera_make.as_deref(), Some("SONY"));
        assert_eq!(document.capture.aperture.as_deref(), Some("f/2.8"));
        assert_eq!(document.editable.rating, Some(4));
        assert_eq!(document.editable.color_label.as_deref(), Some("Red"));
        assert_eq!(
            document.editable.pick_label,
            Some(oxy_domain::PickLabel::Accepted)
        );
        assert_eq!(document.raw.len(), 5);
    }

    #[test]
    fn embedded_none_label_clears_an_earlier_color_projection() {
        let mut metadata = EditableMetadata {
            color_label: Some("Red".into()),
            ..EditableMetadata::default()
        };

        overlay_xmp(&mut metadata, "<rdf:Description xmp:Label='None' />");

        assert_eq!(metadata.color_label, None);
    }

    #[test]
    fn prefers_structural_chroma_subsampling_and_normalizes_exif_values() {
        let tags = vec![
            Tag::new("EXIF", "YCbCrSubSampling", "2 1"),
            Tag::new("JPEG", "ChromaSubsampling", "4:2:0"),
        ];
        let document = document_from_tags(&tags, Path::new("photo.jpg"), None);
        assert_eq!(
            document.capture.chroma_subsampling.as_deref(),
            Some("4:2:0")
        );

        let exif_only = document_from_tags(
            &[Tag::new("EXIF", "YCbCrSubSampling", "2, 1")],
            Path::new("photo.jpg"),
            None,
        );
        assert_eq!(
            exif_only.capture.chroma_subsampling.as_deref(),
            Some("4:2:2")
        );
    }

    #[test]
    fn reads_chroma_subsampling_from_repository_hif() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        let document = NativeMetadataReader.read(&path, None).unwrap();
        assert_eq!(
            document.capture.chroma_subsampling.as_deref(),
            Some("4:2:2")
        );
        assert_eq!(document.capture.color_temperature.as_deref(), Some("Auto"));
        assert_eq!(document.capture.tint.as_deref(), Some("0"));
        assert_eq!(
            document.capture.dynamic_range_optimizer.as_deref(),
            Some("Lv5")
        );
    }

    #[test]
    fn reads_chroma_subsampling_from_jpeg_frame() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.jpg");
        let sof = [8, 0, 16, 0, 16, 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1];
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xc0];
        jpeg.extend_from_slice(&u16::try_from(sof.len() + 2).unwrap().to_be_bytes());
        jpeg.extend_from_slice(&sof);
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        fs::write(&path, jpeg).unwrap();

        let document = NativeMetadataReader.read(&path, None).unwrap();
        assert_eq!(
            document.capture.chroma_subsampling.as_deref(),
            Some("4:2:0")
        );
    }
}
