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
