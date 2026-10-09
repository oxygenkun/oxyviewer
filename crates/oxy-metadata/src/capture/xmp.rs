use super::{
    TagLookup, normalize_exposure_compensation, normalize_focal_length, trim_fraction, trim_seconds,
};
use oxy_domain::CaptureMetadata;
use oxy_metadata_parser::Tag;

/// Overlay only corresponding, present XMP values. Camera-only fields retain
/// their EXIF/MakerNote value when a sidecar contains only editable markings.
pub(crate) fn overlay(capture: &mut CaptureMetadata, tags: &[Tag]) {
    let values = TagLookup::new(tags);
    let value = |names: &[&str]| {
        names.iter().find_map(|name| {
            values
                .value("XMP", name)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
    };
    if let Some(value) = value(&["FNumber"]) {
        capture.aperture = Some(format!("f/{}", trim_fraction(value)));
    }
    if let Some(value) = value(&["ExposureTime"]) {
        capture.exposure_time = Some(format!("{} s", trim_seconds(value)));
    }
    if let Some(value) = value(&["FocalLength"]) {
        capture.focal_length = Some(normalize_focal_length(value));
    }
    if let Some(value) = value(&["ISOSpeedRatings", "PhotographicSensitivity", "ISO"]) {
        capture.iso = Some(value.to_owned());
    }
    if let Some(value) = value(&["ExposureBiasValue", "ExposureCompensation"]) {
        capture.exposure_compensation = Some(normalize_exposure_compensation(value));
    }
    if let Some(value) = value(&["DateTimeOriginal"]) {
        capture.captured_at = Some(value.to_owned());
    }
    if let Some(value) = value(&["Make"]) {
        capture.camera_make = Some(value.to_owned());
    }
    if let Some(value) = value(&["Model"]) {
        capture.camera_model = Some(value.to_owned());
    }
    if let Some(value) = value(&["LensMake"]) {
        capture.lens_make = Some(value.to_owned());
    }
    if let Some(value) = value(&["LensModel", "Lens"]) {
        capture.lens_model = Some(value.to_owned());
    }
    if let Some(value) = value(&["Temperature", "ColorTemperature"])
        && let Ok(kelvin) = value.parse::<u32>()
        && kelvin > 0
    {
        capture.color_temperature = Some(format!("{kelvin} K"));
    }
    // Camera Raw tint is already a signed adjustment. It does not share a
    // camera vendor's MakerNote scale or green/magenta sign convention.
    if let Some(value) = value(&["Tint"])
        && let Ok(tint) = value.parse::<i32>()
    {
        capture.tint = Some(tint.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn corresponding_embedded_xmp_values_override_exif_and_maker_notes() {
        let tags = [
            Tag::new("EXIF", "Make", "SONY"),
            Tag::new("EXIF", "Model", "Camera"),
            Tag::new("EXIF", "FNumber", "2.8"),
            Tag::new("EXIF", "ExposureTime", "1/100"),
            Tag::new("EXIF", "FocalLength", "50 mm"),
            Tag::new("EXIF", "ISO", "100"),
            Tag::new("EXIF", "ExposureCompensation", "0"),
            Tag::new("EXIF", "DateTimeOriginal", "2020:01:01 10:00:00"),
            Tag::new("EXIF", "LensMake", "Original lens make"),
            Tag::new("EXIF", "LensModel", "Original lens"),
            Tag::new("MakerNotes", "ColorTemperature", "5600"),
            Tag::new("MakerNotes", "ColorCompensationFilter", "-2"),
            Tag::new("XMP", "Make", "Edited make"),
            Tag::new("XMP", "Model", "Edited camera"),
            Tag::new("XMP", "FNumber", "40/10"),
            Tag::new("XMP", "ExposureTime", "1/250"),
            Tag::new("XMP", "FocalLength", "85/1"),
            Tag::new("XMP", "ISOSpeedRatings", "400"),
            Tag::new("XMP", "ExposureBiasValue", "-1/3"),
            Tag::new("XMP", "DateTimeOriginal", "2026-10-09T14:30:00+08:00"),
            Tag::new("XMP", "LensMake", "Edited lens make"),
            Tag::new("XMP", "Lens", "Edited lens"),
            Tag::new("XMP", "Temperature", "6500"),
            Tag::new("XMP", "Tint", "12"),
        ];
        let capture = crate::capture::from_tags(&tags, Path::new("photo.arw"));

        assert_eq!(capture.camera_make.as_deref(), Some("Edited make"));
        assert_eq!(capture.camera_model.as_deref(), Some("Edited camera"));
        assert_eq!(capture.aperture.as_deref(), Some("f/4"));
        assert_eq!(capture.exposure_time.as_deref(), Some("1/250 s"));
        assert_eq!(capture.focal_length.as_deref(), Some("85 mm"));
        assert_eq!(capture.iso.as_deref(), Some("400"));
        assert_eq!(capture.exposure_compensation.as_deref(), Some("-0.33 EV"));
        assert_eq!(
            capture.captured_at.as_deref(),
            Some("2026-10-09T14:30:00+08:00")
        );
        assert_eq!(capture.lens_make.as_deref(), Some("Edited lens make"));
        assert_eq!(capture.lens_model.as_deref(), Some("Edited lens"));
        assert_eq!(capture.color_temperature.as_deref(), Some("6500 K"));
        assert_eq!(capture.tint.as_deref(), Some("12"));
    }

    #[test]
    fn absent_empty_or_invalid_xmp_capture_values_keep_camera_values() {
        let original = CaptureMetadata {
            camera_model: Some("Camera".into()),
            color_temperature: Some("5600 K".into()),
            tint: Some("-2 (G)".into()),
            ..CaptureMetadata::default()
        };
        for (temperature, tint) in [("", ""), ("0", "invalid"), ("invalid", "1.5")] {
            let mut capture = original.clone();
            overlay(
                &mut capture,
                &[
                    Tag::new("XMP", "Rating", "4"),
                    Tag::new("XMP", "Model", " "),
                    Tag::new("XMP", "Temperature", temperature),
                    Tag::new("XMP", "Tint", tint),
                ],
            );
            assert_eq!(capture, original);
        }

        let mut capture = original;
        overlay(&mut capture, &[Tag::new("XMP", "Tint", "0")]);
        assert_eq!(capture.tint.as_deref(), Some("0"));
        assert_eq!(capture.color_temperature.as_deref(), Some("5600 K"));
    }
}
