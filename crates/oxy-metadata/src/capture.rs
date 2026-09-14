//! Normalized capture metadata, routed independently by container and camera vendor.

use oxy_domain::CaptureMetadata;
use oxy_metadata_parser::Tag;
use std::path::Path;

mod format;
mod vendor;

pub(super) struct TagLookup<'a> {
    tags: &'a [Tag],
}

impl<'a> TagLookup<'a> {
    fn new(tags: &'a [Tag]) -> Self {
        Self { tags }
    }

    pub(super) fn value(&self, group: &str, name: &str) -> Option<&'a str> {
        self.tags
            .iter()
            .find(|tag| tag.group == group && tag.name == name)
            .map(|tag| tag.value.as_str())
    }
}

pub(super) fn from_tags(tags: &[Tag], path: &Path) -> CaptureMetadata {
    let values = TagLookup::new(tags);
    let image_type = format::ImageType::from_path(path);
    let mut capture = CaptureMetadata {
        aperture: values
            .value("EXIF", "FNumber")
            .or_else(|| values.value("Composite", "Aperture"))
            .map(|value| format!("f/{}", trim_fraction(value))),
        exposure_time: values
            .value("EXIF", "ExposureTime")
            .or_else(|| values.value("Composite", "ShutterSpeed"))
            .map(|value| format!("{} s", trim_seconds(value))),
        focal_length: values
            .value("EXIF", "FocalLength")
            .map(normalize_focal_length),
        iso: values.value("EXIF", "ISO").map(str::to_owned),
        exposure_compensation: values
            .value("EXIF", "ExposureCompensation")
            .map(normalize_exposure_compensation),
        captured_at: values.value("EXIF", "DateTimeOriginal").map(str::to_owned),
        camera_make: values.value("EXIF", "Make").map(str::to_owned),
        camera_model: values.value("EXIF", "Model").map(str::to_owned),
        lens_make: values.value("EXIF", "LensMake").map(str::to_owned),
        lens_model: values.value("EXIF", "LensModel").map(str::to_owned),
        ..CaptureMetadata::default()
    };

    format::apply(image_type, &values, &mut capture);
    vendor::apply(
        vendor::CameraVendor::from_make(capture.camera_make.as_deref()),
        image_type,
        &values,
        &mut capture,
    );
    capture
}

pub(super) fn is_heif_path(path: &Path) -> bool {
    format::ImageType::from_path(path) == format::ImageType::Heif
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
    fn routes_container_values_by_image_type() {
        let tags = [
            Tag::new("JPEG", "ChromaSubsampling", "4:2:0"),
            Tag::new("HEIF", "ChromaFormat", "4:2:2"),
        ];
        assert_eq!(
            from_tags(&tags, Path::new("photo.jpg"))
                .chroma_subsampling
                .as_deref(),
            Some("4:2:0")
        );
        assert_eq!(
            from_tags(&tags, Path::new("photo.hif"))
                .chroma_subsampling
                .as_deref(),
            Some("4:2:2")
        );
    }

    #[test]
    fn does_not_apply_sony_maker_notes_to_other_vendors() {
        let sony_tags = [
            Tag::new("EXIF", "Make", "SONY"),
            Tag::new("MakerNotes", "ColorTemperature", "5600"),
            Tag::new("MakerNotes", "ColorCompensationFilter", "-2"),
            Tag::new("MakerNotes", "DynamicRangeOptimizer", "2"),
        ];
        let sony = from_tags(&sony_tags, Path::new("photo.arw"));
        assert_eq!(sony.color_temperature.as_deref(), Some("5600 K"));
        assert_eq!(sony.tint.as_deref(), Some("-2 (G)"));
        assert_eq!(
            sony.dynamic_range_optimizer.as_deref(),
            Some("Advanced Auto")
        );

        let mut canon_tags = sony_tags;
        canon_tags[0] = Tag::new("EXIF", "Make", "Canon");
        let canon = from_tags(&canon_tags, Path::new("photo.cr3"));
        // ColorTemperature is a shared MakerNotes tag name; the Sony-only
        // tint and DRO tags must not leak into the Canon mapping.
        assert_eq!(canon.color_temperature.as_deref(), Some("5600 K"));
        assert_eq!(canon.tint, None);
        assert_eq!(canon.dynamic_range_optimizer, None);
    }
}
