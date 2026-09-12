//! JPEG embedded-preview eligibility; independent of execution and cache ownership.
#[cfg(test)]
use crate::cache::DisplayDimensions;
use crate::{delivery::THUMBNAIL_EDGE, formats::jpeg::Header};

pub(crate) fn embedded_is_eligible(primary: &Header, candidate: &Header) -> bool {
    let (Some(source), Some(candidate_dimensions)) = (primary.dimensions, candidate.dimensions)
    else {
        return false;
    };
    let target = source.width.max(source.height).min(THUMBNAIL_EDGE);
    let software = primary
        .exif
        .software
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let edited = [
        "adobe",
        "photoshop",
        "lightroom",
        "capture one",
        "gimp",
        "affinity",
        "darktable",
        "dxo",
        "luminar",
        "paint",
        "image magick",
        "imagemagick",
    ]
    .iter()
    .any(|name| software.contains(name));
    // Camera MPF thumbnails have no independent orientation or use the parent
    // orientation. An explicit disagreement cannot be resolved by dimensions.
    let orientation_matches = candidate.exif.orientation.is_none()
        || candidate.exif.orientation == primary.exif.orientation;
    // Without a child ICC only an explicitly sRGB parent with no conflicting
    // ICC is safe to inherit. Unknown/wide-gamut parents use their primary.
    let color_known = candidate.has_icc || (primary.exif.srgb && !primary.has_icc);
    let ratio_difference = (i64::from(source.width) * i64::from(candidate_dimensions.height)
        - i64::from(source.height) * i64::from(candidate_dimensions.width))
    .unsigned_abs();
    let ratio_reference = u64::from(source.width) * u64::from(candidate_dimensions.height);
    primary.complete
        && candidate.complete
        && !primary.has_edit_metadata
        && !edited
        && orientation_matches
        && color_known
        && primary.exif.orientation.is_some()
        && candidate_dimensions.width.max(candidate_dimensions.height) >= target
        && ratio_reference > 0
        && ratio_difference * 1000 <= ratio_reference * 5
}

#[cfg(test)]
mod tests {
    use super::*;
    fn header(width: u32, height: u32) -> Header {
        Header {
            dimensions: Some(DisplayDimensions { width, height }),
            complete: true,
            exif: oxy_metadata_parser::jpeg_preview::ExifPresentation {
                orientation: Some(8),
                srgb: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    #[test]
    fn quality_color_geometry_and_edit_history_are_independent() {
        let mut source = header(7008, 4672);
        let mut candidate = header(1616, 1080);
        assert!(embedded_is_eligible(&source, &candidate));
        assert!(!embedded_is_eligible(&source, &header(160, 120)));
        assert!(!embedded_is_eligible(&source, &header(1920, 1080)));
        candidate.exif.orientation = Some(1);
        assert!(!embedded_is_eligible(&source, &candidate));
        candidate.exif.orientation = None;
        source.exif.srgb = false;
        assert!(!embedded_is_eligible(&source, &candidate));
        candidate.has_icc = true;
        assert!(embedded_is_eligible(&source, &candidate));
        source.exif.software = Some("Adobe Photoshop".into());
        assert!(!embedded_is_eligible(&source, &candidate));
        source.exif.software = Some("Unknown camera firmware".into());
        assert!(embedded_is_eligible(&source, &candidate));
        source.has_edit_metadata = true;
        assert!(!embedded_is_eligible(&source, &candidate));
    }
}
