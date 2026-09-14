use super::super::{TagLookup, format::ImageType};
use oxy_domain::CaptureMetadata;

pub(super) fn apply(_image_type: ImageType, values: &TagLookup<'_>, capture: &mut CaptureMetadata) {
    capture.color_temperature = values
        .value("MakerNotes", "ColorTemperature")
        .and_then(normalize_color_temperature);
    capture.tint = values
        .value("MakerNotes", "WBShiftGM")
        .and_then(normalize_tint);
    capture.dynamic_range_optimizer = values
        .value("MakerNotes", "AutoLightingOptimizer")
        .and_then(normalize_auto_lighting_optimizer);
}

/// Canon CameraSettings ColorTemperature is a plain Kelvin value; 0 or
/// negative means the camera did not record one.
fn normalize_color_temperature(value: &str) -> Option<String> {
    let value = value.trim();
    match value.parse::<i32>() {
        Ok(kelvin) if kelvin > 0 => Some(format!("{kelvin} K")),
        _ => None,
    }
}

/// Canon WBShiftGM: positive shifts toward yellow/green, negative toward
/// magenta (opposite sign convention to Sony's ColorCompensationFilter).
fn normalize_tint(value: &str) -> Option<String> {
    let tint = value.trim().parse::<i32>().ok()?;
    Some(match tint.cmp(&0) {
        std::cmp::Ordering::Less => format!("{tint} (M)"),
        std::cmp::Ordering::Equal => "0".to_owned(),
        std::cmp::Ordering::Greater => format!("+{tint} (G)"),
    })
}

/// Canon's DRO equivalent is Auto Lighting Optimizer; the parser already
/// prints human-readable levels (Standard/Low/Strong/Off).
fn normalize_auto_lighting_optimizer(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.starts_with("Unknown") {
        return None;
    }
    Some(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::from_tags;
    use oxy_metadata_parser::Tag;
    use std::path::Path;

    #[test]
    fn maps_canon_maker_notes_to_capture_fields() {
        let tags = [
            Tag::new("EXIF", "Make", "Canon"),
            Tag::new("MakerNotes", "ColorTemperature", "4500"),
            Tag::new("MakerNotes", "WBShiftGM", "2"),
            Tag::new("MakerNotes", "AutoLightingOptimizer", "Standard"),
        ];
        let capture = from_tags(&tags, Path::new("photo.cr3"));
        assert_eq!(capture.color_temperature.as_deref(), Some("4500 K"));
        assert_eq!(capture.tint.as_deref(), Some("+2 (G)"));
        assert_eq!(capture.dynamic_range_optimizer.as_deref(), Some("Standard"));
    }

    #[test]
    fn rejects_missing_canon_color_temperature() {
        for value in ["0", "-1", ""] {
            assert_eq!(normalize_color_temperature(value), None);
        }
    }

    #[test]
    fn canon_tint_signs_toward_green_and_magenta() {
        assert_eq!(normalize_tint("-3").as_deref(), Some("-3 (M)"));
        assert_eq!(normalize_tint("0").as_deref(), Some("0"));
        assert_eq!(normalize_tint("2").as_deref(), Some("+2 (G)"));
    }

    #[test]
    fn drops_unknown_auto_lighting_optimizer() {
        assert_eq!(normalize_auto_lighting_optimizer("Unknown (7)"), None);
        assert_eq!(normalize_auto_lighting_optimizer(""), None);
        assert_eq!(
            normalize_auto_lighting_optimizer("Off").as_deref(),
            Some("Off")
        );
    }
}
