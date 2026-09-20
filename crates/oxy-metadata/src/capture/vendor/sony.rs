use super::super::{TagLookup, format::ImageType};
use oxy_domain::CaptureMetadata;

pub(super) fn apply(_image_type: ImageType, values: &TagLookup<'_>, capture: &mut CaptureMetadata) {
    capture.color_temperature = values
        .value("MakerNotes", "ColorTemperature")
        .and_then(normalize_color_temperature);
    capture.tint = values
        .value("MakerNotes", "ColorCompensationFilter")
        .and_then(normalize_tint);
    capture.dynamic_range_optimizer = values
        .value("MakerNotes", "DynamicRangeOptimizer")
        .and_then(normalize_dynamic_range_optimizer);
    capture.release_mode = values
        .value("MakerNotes", "ReleaseMode")
        .and_then(normalize_release_mode);
    capture.sequence_number = values
        .value("MakerNotes", "SequenceNumber")
        .and_then(normalize_sequence_number);
}

fn normalize_color_temperature(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || matches!(value, "65535" | "4294967295") {
        return None;
    }
    if value.eq_ignore_ascii_case("auto") || value == "0" {
        return Some("Auto".to_owned());
    }
    if value.ends_with('K') || value.parse::<u32>().is_err() {
        return Some(value.to_owned());
    }
    Some(format!("{value} K"))
}

fn normalize_tint(value: &str) -> Option<String> {
    let tint = value.trim().parse::<i32>().ok()?;
    Some(match tint.cmp(&0) {
        std::cmp::Ordering::Less => format!("{tint} (G)"),
        std::cmp::Ordering::Equal => "0".to_owned(),
        std::cmp::Ordering::Greater => format!("+{tint} (M)"),
    })
}

fn normalize_dynamic_range_optimizer(value: &str) -> Option<String> {
    let value = value.trim();
    let normalized = match value {
        "0" => "Off",
        "1" => "Standard",
        "2" => "Advanced Auto",
        "3" => "Auto",
        "8" => "Advanced Lv1",
        "9" => "Advanced Lv2",
        "10" => "Advanced Lv3",
        "11" => "Advanced Lv4",
        "12" => "Advanced Lv5",
        "16" => "Lv1",
        "17" => "Lv2",
        "18" => "Lv3",
        "19" => "Lv4",
        "20" => "Lv5",
        "128" | "65535" | "4294967295" | "" => return None,
        other => other,
    };
    Some(normalized.to_owned())
}

fn normalize_release_mode(value: &str) -> Option<String> {
    let normalized = match value.trim() {
        "0" => "Normal",
        "2" => "Continuous",
        "5" => "Exposure Bracketing",
        "6" => "White Balance Bracketing",
        "8" => "DRO Bracketing",
        "65535" | "4294967295" | "" => return None,
        other => other,
    };
    Some(normalized.to_owned())
}

/// Sony's `SequenceNumber` is the 1-based index of a frame inside its burst.
/// `0` marks a single shot and `65535` means the camera did not report an
/// index, so neither takes part in burst grouping.
fn normalize_sequence_number(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>().ok()? {
        0 | 65535 | 4294967295 => None,
        number => Some(number),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::from_tags;
    use oxy_metadata_parser::Tag;
    use std::path::Path;

    #[test]
    fn maps_sony_burst_tags_to_capture_fields() {
        let tags = [
            Tag::new("EXIF", "Make", "SONY"),
            Tag::new("MakerNotes", "ReleaseMode", "2"),
            Tag::new("MakerNotes", "SequenceNumber", "4"),
        ];
        let capture = from_tags(&tags, Path::new("DSC00511.HIF"));
        assert_eq!(capture.release_mode.as_deref(), Some("Continuous"));
        assert_eq!(capture.sequence_number, Some(4));
    }

    #[test]
    fn single_frames_carry_no_sequence_number() {
        let tags = [
            Tag::new("EXIF", "Make", "SONY"),
            Tag::new("MakerNotes", "ReleaseMode", "0"),
            Tag::new("MakerNotes", "SequenceNumber", "0"),
        ];
        let capture = from_tags(&tags, Path::new("DSC02948.ARW"));
        assert_eq!(capture.release_mode.as_deref(), Some("Normal"));
        assert_eq!(capture.sequence_number, None);
    }

    #[test]
    fn drops_unreported_burst_values() {
        for value in ["65535", "4294967295", "", "n/a"] {
            assert_eq!(normalize_sequence_number(value), None);
        }
        for value in ["65535", "4294967295", ""] {
            assert_eq!(normalize_release_mode(value), None);
        }
    }

    #[test]
    fn names_every_known_release_mode() {
        assert_eq!(normalize_release_mode("2").as_deref(), Some("Continuous"));
        assert_eq!(
            normalize_release_mode("5").as_deref(),
            Some("Exposure Bracketing")
        );
        assert_eq!(
            normalize_release_mode("8").as_deref(),
            Some("DRO Bracketing")
        );
    }
}
