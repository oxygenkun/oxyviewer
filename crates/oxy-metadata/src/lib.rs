use fpexif::{ExifParser, data_types::ExifValue};
use oxy_domain::{AssetKind, EditableMetadata, FocusInfo, FocusRegion};
use oxy_fs::sidecar_path;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("embedded metadata writes require the bundled ExifTool worker")]
    EmbeddedWorkerUnavailable,
    #[error("metadata read failed: {0}")]
    Read(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Reads Sony's shooting focus location from any container supported by
/// fpexif (including ARW, JPEG, HEIF/HIF). Unsupported vendors and files that
/// do not contain a valid FocusLocation return `Ok(None)`.
pub fn read_focus_info(
    path: &Path,
    display_dimensions: Option<(u32, u32)>,
) -> Result<Option<FocusInfo>, MetadataError> {
    let exif = ExifParser::new()
        .strict(false)
        .parse_file(path)
        .map_err(|error| MetadataError::Read(error.to_string()))?;
    let Some(maker_notes) = exif.get_maker_notes() else {
        return Ok(None);
    };
    let Some(location) = maker_notes
        .get(&0x2027)
        .filter(|tag| tag.tag_name == Some("FocusLocation"))
        .and_then(short_values)
    else {
        return Ok(None);
    };
    if location.len() < 4 {
        return Ok(None);
    }
    let [width, height, center_x, center_y] = [
        u32::from(location[0]),
        u32::from(location[1]),
        u32::from(location[2]),
        u32::from(location[3]),
    ];
    if width == 0 || height == 0 || center_x > width || center_y > height {
        return Ok(None);
    }

    let frame_size = maker_notes
        .get(&0x2037)
        .filter(|tag| tag.tag_name == Some("FocusFrameSize"))
        .and_then(focus_frame_size);
    let exif_orientation = exif
        .get_tag_by_name("Orientation")
        .and_then(|value| match value {
            ExifValue::Short(values) => values.first().copied(),
            _ => None,
        });
    let orientation = exif_orientation.unwrap_or_else(|| {
        // HEIF commonly stores orientation as an item transform rather than an
        // EXIF tag. If the display dimensions prove the axes are swapped, use
        // Sony's usual clockwise portrait transform.
        display_dimensions
            .filter(|(display_width, display_height)| {
                (width > height) != (display_width > display_height)
            })
            .map(|_| 6)
            .unwrap_or(1)
    });

    Ok(Some(orient_focus_info(
        width,
        height,
        center_x,
        center_y,
        frame_size,
        orientation,
    )))
}

fn short_values(tag: &fpexif::makernotes::MakerNoteTag) -> Option<&[u16]> {
    match tag.raw_value.as_ref().unwrap_or(&tag.value) {
        ExifValue::Short(values) => Some(values),
        _ => None,
    }
}

fn focus_frame_size(tag: &fpexif::makernotes::MakerNoteTag) -> Option<(u32, u32)> {
    match tag.raw_value.as_ref().unwrap_or(&tag.value) {
        ExifValue::Short(values) if values.len() >= 3 && values[2] != 0 => {
            Some((u32::from(values[0]), u32::from(values[1])))
        }
        ExifValue::Ascii(value) => {
            let (width, height) = value.split_once('x')?;
            let width = width.trim().parse().ok()?;
            let height = height.trim().parse().ok()?;
            (width > 0 && height > 0).then_some((width, height))
        }
        _ => None,
    }
}

fn orient_focus_info(
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
    kind: AssetKind,
    metadata: &EditableMetadata,
) -> Result<PathBuf, MetadataError> {
    if kind != AssetKind::Raw {
        return Err(MetadataError::EmbeddedWorkerUnavailable);
    }
    write_raw_sidecar(path, metadata)
}

pub fn write_raw_sidecar(
    raw_path: &Path,
    metadata: &EditableMetadata,
) -> Result<PathBuf, MetadataError> {
    let destination = sidecar_path(raw_path);
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
}
