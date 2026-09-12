//! JPEG thumbnail requirements and candidate planning, independent of I/O.
#[cfg(test)]
use crate::media_source::PixelDimensions;
use crate::{
    cache::{
        ArtifactRequirement, CacheRequest, ColorRequirement, DetailRequirement,
        OrientationRequirement, PresentationRequirement, SharpeningState,
    },
    delivery::{Delivery, THUMBNAIL_EDGE, THUMBNAIL_LIMITS},
    formats::jpeg::Header,
    pipeline::artifact::ArtifactCache,
    policy::JPEG_THUMBNAIL,
};
use oxy_metadata_parser::jpeg_preview::JpegRange;

pub(super) fn thumbnail_request(artifacts: &ArtifactCache) -> CacheRequest {
    artifacts.request(
        DetailRequirement::Display {
            min_long_edge: THUMBNAIL_EDGE,
        },
        ArtifactRequirement::BoundedThumbnail {
            target: JPEG_THUMBNAIL,
        },
        PresentationRequirement {
            orientation: OrientationRequirement::DisplayCorrect,
            color: ColorRequirement::Any,
            sharpening: SharpeningState::None,
        },
        false,
    )
}

pub(super) fn can_deliver_original(primary: &Header, byte_size: u64) -> bool {
    primary.complete
        && primary.encoded_dimensions.is_some_and(|dimensions| {
            THUMBNAIL_LIMITS.classify(dimensions.0, byte_size) == Delivery::Direct
        })
}

pub(super) fn plan_embedded_candidates(
    primary: &Header,
    mut candidates: Vec<(JpegRange, Header)>,
) -> Vec<(JpegRange, Header)> {
    candidates.retain(|(_, candidate)| embedded_is_eligible(primary, candidate));
    candidates.sort_by_key(|(range, candidate)| {
        let dimensions = candidate
            .encoded_dimensions
            .expect("eligible candidate has dimensions");
        (
            u64::from(dimensions.0.width) * u64::from(dimensions.0.height),
            range.length,
        )
    });
    // Bound full materializations and decode attempts even for corrupt MP payloads.
    candidates.truncate(3);
    candidates
}

fn embedded_is_eligible(primary: &Header, candidate: &Header) -> bool {
    let (Some(source), Some(candidate_dimensions)) =
        (primary.encoded_dimensions, candidate.encoded_dimensions)
    else {
        return false;
    };
    let source = source.0;
    let candidate_dimensions = candidate_dimensions.0;
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
            encoded_dimensions: Some(oxy_domain::EncodedDimensions(PixelDimensions {
                width,
                height,
            })),
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
    fn candidate_plan_filters_before_ranking_and_bounds_fallback_attempts() {
        let source = header(6000, 4000);
        let candidates = [
            (1, 500, 1200, 800),
            (2, 900, 900, 600),
            (3, 700, 900, 600),
            (4, 800, 600, 400),
            (5, 1, 160, 120),
            (6, 1, 1920, 1080),
        ]
        .into_iter()
        .map(|(offset, length, width, height)| {
            (JpegRange { offset, length }, header(width, height))
        })
        .collect();
        let plan = plan_embedded_candidates(&source, candidates);
        assert_eq!(
            plan.iter()
                .map(|(range, _)| range.offset)
                .collect::<Vec<_>>(),
            [4, 3, 2]
        );
    }

    #[test]
    fn direct_original_requires_complete_header_and_bounded_delivery() {
        let mut source = header(512, 341);
        assert!(can_deliver_original(&source, 1024));
        assert!(!can_deliver_original(&source, 2 * 1024 * 1024 + 1));
        source.complete = false;
        assert!(!can_deliver_original(&source, 1024));
        assert!(!can_deliver_original(&header(513, 342), 1024));
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
