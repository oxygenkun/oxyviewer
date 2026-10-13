//! Portable XMP value normalization shared by reads and application edits.
use crate::types::{EditableMetadata, PickLabel};

pub fn editable_from_sidecar_xml(xml: &str) -> EditableMetadata {
    let rating_value = xmp_value(xml, "Rating");
    let pick_label = xmp_prefixed_value(xml, "digiKam", "PickLabel")
        .as_deref()
        .and_then(parse_pick_label)
        .or_else(|| {
            rating_value
                .as_deref()
                .filter(|value| is_rejected_xmp_rating(value))
                .map(|_| PickLabel::Rejected)
        });
    EditableMetadata {
        rating: rating_value.as_deref().and_then(parse_xmp_rating),
        color_label: xmp_value(xml, "Label").and_then(normalize_color_label),
        pick_label,
        keywords: xmp_array_values(xml, "dc", "subject"),
        hierarchical_keywords: xmp_array_values(xml, "lr", "hierarchicalSubject"),
        ..EditableMetadata::default()
    }
}

pub fn xmp_value(xml: &str, name: &str) -> Option<String> {
    xmp_prefixed_value(xml, "xmp", name)
}

pub fn xmp_prefixed_value(xml: &str, prefix: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let attribute = format!("{prefix}:{name}={quote}");
        if let Some(start) = xml.find(&attribute) {
            let value = &xml[start + attribute.len()..];
            return value.find(quote).map(|end| unescape_xml(&value[..end]));
        }
    }
    let open = format!("<{prefix}:{name}>");
    let close = format!("</{prefix}:{name}>");
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

pub fn normalize_color_label(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return None;
    }
    ["Red", "Yellow", "Green", "Blue", "Purple"]
        .into_iter()
        .find(|label| label.eq_ignore_ascii_case(value))
        .map(str::to_owned)
        .or_else(|| Some(value.to_owned()))
}

pub fn parse_pick_label(value: &str) -> Option<PickLabel> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "rejected" => Some(PickLabel::Rejected),
        "2" | "pending" => Some(PickLabel::Pending),
        "3" | "accepted" | "picked" => Some(PickLabel::Accepted),
        _ => None,
    }
}

pub fn parse_xmp_rating(value: &str) -> Option<u8> {
    value
        .trim()
        .parse::<u8>()
        .ok()
        .filter(|rating| *rating <= 5)
}

pub fn is_rejected_xmp_rating(value: &str) -> bool {
    value.trim() == "-1"
}

pub fn xmp_array_values(xml: &str, prefix: &str, name: &str) -> Vec<String> {
    let element_open = format!("<{prefix}:{name}");
    let element_close = format!("</{prefix}:{name}>");
    let Some(element_start) = xml.find(&element_open) else {
        return Vec::new();
    };
    let Some(content_start) = xml[element_start..]
        .find('>')
        .map(|offset| element_start + offset + 1)
    else {
        return Vec::new();
    };
    let Some(content_end) = xml[content_start..]
        .find(&element_close)
        .map(|offset| content_start + offset)
    else {
        return Vec::new();
    };
    let mut values = Vec::new();
    let mut remaining = &xml[content_start..content_end];
    while let Some(item_start) = remaining.find("<rdf:li") {
        let Some(value_start) = remaining[item_start..]
            .find('>')
            .map(|offset| item_start + offset + 1)
        else {
            break;
        };
        let Some(value_end) = remaining[value_start..].find("</rdf:li>") else {
            break;
        };
        values.push(unescape_xml(
            remaining[value_start..value_start + value_end].trim(),
        ));
        remaining = &remaining[value_start + value_end + "</rdf:li>".len()..];
    }
    values
}
