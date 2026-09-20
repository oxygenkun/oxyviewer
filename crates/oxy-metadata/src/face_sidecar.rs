//! Structured portable face facts, preserving every unrelated byte in an existing XMP packet.
use crate::{MetadataError, MetadataReader, NativeMetadataReader};
use oxy_domain::{FaceDecision, NormalizedRect, PortableFaceFact, PortableFaceFacts};
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
    name::ResolveResult,
    reader::NsReader,
};
use std::{collections::HashMap, fs, path::Path};

const NS: &[u8] = b"https://oxyviewer.app/ns/faces/1.0/";
const RDF: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";

fn invalid(error: impl std::fmt::Display) -> MetadataError {
    MetadataError::Read(error.to_string())
}

fn attributes(
    element: &BytesStart<'_>,
    reader: &Reader<&[u8]>,
) -> Result<HashMap<String, String>, MetadataError> {
    element
        .attributes()
        .map(|attribute| {
            let attribute = attribute.map_err(invalid)?;
            Ok((
                String::from_utf8_lossy(attribute.key.local_name().as_ref()).into_owned(),
                attribute
                    .decode_and_unescape_value(reader.decoder())
                    .map_err(invalid)?
                    .into_owned(),
            ))
        })
        .collect()
}

struct Parsed {
    facts: PortableFaceFacts,
    range: Option<std::ops::Range<usize>>,
    insert_at: Option<usize>,
}

fn parse(xml: &str) -> Result<Parsed, MetadataError> {
    let mut reader = NsReader::from_str(xml);
    let mut depth = 0usize;
    let mut property = None;
    let mut range = None;
    let mut insert_at = None;
    let mut facts = Vec::new();
    loop {
        let position = reader.buffer_position() as usize;
        let event = reader.read_event().map_err(invalid)?;
        match event {
            Event::Start(element) => {
                let (ns, name) = reader.resolve_element(element.name());
                if matches!(ns, ResolveResult::Bound(value) if value.as_ref()==NS)
                    && name.as_ref() == b"FaceDecisions"
                {
                    if property.is_some() || range.is_some() {
                        return Err(invalid("duplicate face namespace"));
                    }
                    let attributes = attributes(&element, &reader)?;
                    if attributes.get("version").map(String::as_str) != Some("1") {
                        return Err(invalid("unsupported face sidecar version"));
                    }
                    property = Some((position, depth));
                }
                depth += 1;
            }
            Event::Empty(element) => {
                let (ns, name) = reader.resolve_element(element.name());
                if matches!(ns, ResolveResult::Bound(value) if value.as_ref()==NS)
                    && name.as_ref() == b"Decision"
                {
                    if property.is_none() {
                        return Err(invalid("face decision outside its property"));
                    }
                    let attrs = attributes(&element, &reader)?;
                    let get = |key: &str| {
                        attrs
                            .get(key)
                            .cloned()
                            .ok_or_else(|| invalid(format!("missing face {key}")))
                    };
                    let person = || get("personId");
                    let decision = match get("kind")?.as_str() {
                        "confirmPerson" => Some(FaceDecision::ConfirmPerson {
                            person_id: person()?,
                        }),
                        "rejectPerson" => Some(FaceDecision::RejectPerson {
                            person_id: person()?,
                        }),
                        "notFace" => Some(FaceDecision::NotFace),
                        "deleted" => None,
                        _ => return Err(invalid("unsupported face decision")),
                    };
                    let float = |key| get(key)?.parse::<f32>().map_err(invalid);
                    let region = NormalizedRect {
                        x: float("x")?,
                        y: float("y")?,
                        width: float("width")?,
                        height: float("height")?,
                    };
                    if !region.is_display_normalized() {
                        return Err(invalid("invalid face region"));
                    }
                    let id = get("id")?;
                    let revision = get("revision")?;
                    if id.is_empty()
                        || revision.is_empty()
                        || facts.iter().any(|fact: &PortableFaceFact| fact.id == id)
                    {
                        return Err(invalid("invalid face identity"));
                    }
                    facts.push(PortableFaceFact {
                        person_tag_path: attrs.get("personTagPath").cloned(),
                        id,
                        revision,
                        region,
                        decision,
                        person_name: attrs.get("personName").cloned(),
                    });
                }
            }
            Event::End(element) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| invalid("invalid XML depth"))?;
                let (ns, name) = reader.resolve_element(element.name());
                if matches!(ns, ResolveResult::Bound(value) if value.as_ref()==RDF)
                    && name.as_ref() == b"RDF"
                {
                    insert_at = Some(position);
                }
                if let Some((start, level)) = property
                    && depth == level
                {
                    range = Some(start..reader.buffer_position() as usize);
                    property = None;
                }
            }
            Event::DocType(_) => return Err(invalid("DTD is not supported for face sidecars")),
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 || property.is_some() {
        return Err(invalid("incomplete XMP"));
    }
    facts.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Parsed {
        facts: PortableFaceFacts { facts },
        range,
        insert_at,
    })
}

fn property(facts: &PortableFaceFacts) -> String {
    let escape = crate::escape_xml;
    let mut value = String::from(
        "<oxy:FaceDecisions xmlns:oxy=\"https://oxyviewer.app/ns/faces/1.0/\" xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" oxy:version=\"1\"><rdf:Seq>",
    );
    for fact in &facts.facts {
        let kind = match &fact.decision {
            Some(FaceDecision::ConfirmPerson { .. }) => "confirmPerson",
            Some(FaceDecision::RejectPerson { .. }) => "rejectPerson",
            Some(FaceDecision::NotFace) => "notFace",
            None => "deleted",
        };
        let person = fact
            .decision
            .as_ref()
            .and_then(FaceDecision::person_id)
            .map(|id| format!(" oxy:personId=\"{}\"", escape(id)))
            .unwrap_or_default();
        let name = fact
            .person_name
            .as_ref()
            .map(|name| format!(" oxy:personName=\"{}\"", escape(name)))
            .unwrap_or_default();
        let tag_path = fact
            .person_tag_path
            .as_ref()
            .map(|path| format!(" oxy:personTagPath=\"{}\"", escape(path)))
            .unwrap_or_default();
        value.push_str(&format!("<rdf:li rdf:parseType=\"Resource\"><oxy:Decision oxy:id=\"{}\" oxy:revision=\"{}\" oxy:x=\"{}\" oxy:y=\"{}\" oxy:width=\"{}\" oxy:height=\"{}\" oxy:kind=\"{kind}\"{person}{name}{tag_path}/></rdf:li>", escape(&fact.id), escape(&fact.revision), fact.region.x, fact.region.y, fact.region.width, fact.region.height));
    }
    value.push_str("</rdf:Seq></oxy:FaceDecisions>");
    value
}

pub fn read_face_sidecar(asset: &Path) -> Result<PortableFaceFacts, MetadataError> {
    match fs::read_to_string(oxy_fs::sidecar_path(asset)) {
        Ok(xml) => Ok(parse(&xml)?.facts),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(PortableFaceFacts::default())
        }
        Err(error) => Err(error.into()),
    }
}

/// Compare-and-write under the same lock as rating and keyword updates.
pub fn write_face_sidecar(
    asset: &Path,
    expected: &PortableFaceFacts,
    desired: &PortableFaceFacts,
) -> Result<(), MetadataError> {
    let destination = oxy_fs::sidecar_path(asset);
    let _guard = crate::sidecar_write_lock(&destination)
        .lock()
        .map_err(|_| invalid("sidecar write lock poisoned"))?;
    let original = match fs::read_to_string(&destination) {
        Ok(xml) => Some(xml),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let xml = original.clone().unwrap_or_else(|| {
        let editable = NativeMetadataReader
            .read(asset, None)
            .map(|document| document.editable)
            .unwrap_or_default();
        crate::serialize_xmp(&editable)
    });
    let parsed = parse(&xml)?;
    if parsed.facts != *expected {
        return Err(invalid("face sidecar changed during synchronization"));
    }
    let replacement = property(desired);
    let next = if let Some(range) = parsed.range {
        format!(
            "{}{}{}",
            &xml[..range.start],
            replacement,
            &xml[range.end..]
        )
    } else {
        let at = parsed
            .insert_at
            .ok_or_else(|| invalid("XMP has no RDF container"))?;
        format!(
            "{}<rdf:Description xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">{replacement}</rdf:Description>{}",
            &xml[..at],
            &xml[at..]
        )
    };
    if parse(&next)?.facts != *desired {
        return Err(invalid("face sidecar roundtrip failed"));
    }
    // Other applications do not share our lock. Detect edits made during preparation.
    let current = match fs::read_to_string(&destination) {
        Ok(xml) => Some(xml),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if current != original {
        return Err(invalid("sidecar changed during synchronization"));
    }
    crate::write_sidecar_atomically(&destination, &next)?;
    if read_face_sidecar(asset)? != *desired {
        return Err(invalid("face sidecar verification failed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn facts() -> PortableFaceFacts {
        PortableFaceFacts {
            facts: vec![PortableFaceFact {
                person_tag_path: Some("人物|家人 & 朋友|Alice".into()),
                id: "decision-1".into(),
                revision: "revision-1".into(),
                region: NormalizedRect {
                    x: 0.1,
                    y: 0.2,
                    width: 0.3,
                    height: 0.4,
                },
                decision: Some(FaceDecision::ConfirmPerson {
                    person_id: "person-1".into(),
                }),
                person_name: Some("Alice & <Bob> \"family\"".into()),
            }],
        }
    }
    #[test]
    fn roundtrip_preserves_foreign_metadata_and_detects_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let asset = temp.path().join("photo.jpg");
        fs::write(&asset, b"fixture").unwrap();
        let sidecar = oxy_fs::sidecar_path(&asset);
        let original = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><r:Description xmlns:custom="urn:custom" custom:keep="exactly"><custom:Value>keep me</custom:Value></r:Description></r:RDF></x:xmpmeta>"#;
        fs::write(&sidecar, original).unwrap();
        let expected = PortableFaceFacts::default();
        let desired = facts();
        write_face_sidecar(&asset, &expected, &desired).unwrap();
        assert_eq!(read_face_sidecar(&asset).unwrap(), desired);
        let xml = fs::read_to_string(&sidecar).unwrap();
        assert!(xml.contains("<custom:Value>keep me</custom:Value>"));
        assert!(!xml.contains("embedding"));
        assert!(!xml.contains("observation"));
        assert!(write_face_sidecar(&asset, &expected, &desired).is_err());
        let mut removed = desired.clone();
        removed.facts[0].decision = None;
        removed.facts[0].person_name = None;
        removed.facts[0].person_tag_path = None;
        removed.facts[0].revision = "deletion-1".into();
        write_face_sidecar(&asset, &desired, &removed).unwrap();
        assert_eq!(read_face_sidecar(&asset).unwrap(), removed);
    }
    #[test]
    fn rejects_future_schema_and_dtd_without_overwriting() {
        assert!(parse("<!DOCTYPE x><x/>").is_err());
        assert!(parse(r#"<oxy:FaceDecisions xmlns:oxy="https://oxyviewer.app/ns/faces/1.0/" oxy:version="99"></oxy:FaceDecisions>"#).is_err());
    }
}
