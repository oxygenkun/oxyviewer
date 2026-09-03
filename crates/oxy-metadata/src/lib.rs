use fpexif::{ExifParser, data_types::ExifValue};
use oxy_domain::{AssetKind, EditableMetadata, FocusInfo, FocusRegion};
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
    sync::{Arc, RwLock},
};
use thiserror::Error;

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

/// Stable application boundary for metadata providers. Capture metadata and
/// focus information stay in-process; this configured executable is consulted
/// only for embedded XMP reads and writes.
#[derive(Debug, Clone, Default)]
pub struct MetadataFacade {
    exiftool: Arc<RwLock<Option<PathBuf>>>,
}

impl MetadataFacade {
    pub fn new(exiftool: Option<PathBuf>) -> Self {
        Self {
            exiftool: Arc::new(RwLock::new(exiftool)),
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
        let executable = self.exiftool();
        read_metadata_with_exiftool(path, kind, executable.as_deref())
    }

    pub fn enrich_summaries(
        &self,
        assets: &mut [oxy_domain::AssetSummary],
    ) -> Result<(), MetadataError> {
        let executable = self.exiftool();
        enrich_summaries_with_exiftool(assets, executable.as_deref())
    }

    pub fn patch_metadata(
        &self,
        path: &Path,
        kind: AssetKind,
        patch: &oxy_domain::MetadataPatch,
    ) -> Result<PathBuf, MetadataError> {
        patch_metadata_to_sidecar(path, kind, patch)
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
        patch_embedded(path, &patch, executable.as_deref())
    }

    pub fn exiftool_version(&self) -> Result<String, MetadataError> {
        let executable = self.exiftool();
        probe_exiftool(executable.as_deref())
    }
}

/// Reads editable metadata from an adjacent XMP sidecar first. Without a
/// sidecar, RAW returns empty metadata and other formats try embedded XMP.
pub fn read_metadata(path: &Path, kind: AssetKind) -> Result<EditableMetadata, MetadataError> {
    read_metadata_with_exiftool(path, kind, None)
}

/// Reads editable metadata using a facade-selected ExifTool executable for
/// embedded formats. Passing `None` preserves environment/PATH discovery.
pub fn read_metadata_with_exiftool(
    path: &Path,
    kind: AssetKind,
    exiftool: Option<&Path>,
) -> Result<EditableMetadata, MetadataError> {
    if sidecar_path(path).is_file() || kind == AssetKind::Raw {
        return read_sidecar(path);
    }
    let mut values = read_embedded_batch(std::slice::from_ref(&path.to_path_buf()), exiftool)?;
    Ok(values.remove(path).unwrap_or_default())
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
        .filter(|asset| asset.kind != AssetKind::Raw && !sidecar_path(&asset.path).is_file())
        .map(|asset| asset.path.clone())
        .collect::<Vec<_>>();
    let embedded_values = match read_embedded_batch(&embedded, exiftool) {
        Ok(values) => values,
        Err(MetadataError::EmbeddedWorkerUnavailable) => HashMap::new(),
        Err(error) => return Err(error),
    };
    for asset in assets {
        let metadata = if asset.kind == AssetKind::Raw || sidecar_path(&asset.path).is_file() {
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

fn xmp_value(xml: &str, name: &str) -> Option<String> {
    let attribute = format!("xmp:{name}=\"");
    if let Some(start) = xml.find(&attribute) {
        let value = &xml[start + attribute.len()..];
        return value.find('"').map(|end| unescape_xml(&value[..end]));
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
        command.args(paths);
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
            if let Some(path) = paths.get(index) {
                result.insert(path.clone(), metadata);
            }
        }
    }
    Ok(result)
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

fn normalize_color_label(value: String) -> Option<String> {
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
    let sony_hif = path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("hif"));
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
        serialize_xmp(&EditableMetadata::default())
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

    #[test]
    fn facade_switches_the_embedded_provider_without_affecting_its_contract() {
        let facade = MetadataFacade::default();
        assert_eq!(facade.exiftool(), None);
        let configured = PathBuf::from("/managed/exiftool");
        facade.set_exiftool(Some(configured.clone()));
        assert_eq!(facade.exiftool(), Some(configured));
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
        let focus = read_focus_info(&path, Some((4_672, 7_008)))
            .unwrap()
            .expect("repository HIF fixture has Sony FocusLocation");

        assert_eq!(focus.coordinate_width, 4_672);
        assert_eq!(focus.coordinate_height, 7_008);
        assert_eq!(focus.regions.len(), 1);
        assert_eq!(focus.regions[0].center_x, 2_327);
        assert_eq!(focus.regions[0].center_y, 1_489);
        assert_eq!(focus.regions[0].width, Some(154));
        assert_eq!(focus.regions[0].height, Some(153));
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
