use oxy_domain::{AssetKind, EditableMetadata};
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
    #[error(transparent)]
    Io(#[from] std::io::Error),
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
