//! Adapt cached representations to the thumbnail delivery contract.
use super::{
    artifact::ArtifactCache,
    jpeg_transform::{EncodedJpegThumbnail, encode_jpeg_thumbnail},
};
use crate::{
    MediaError,
    cache::{
        ArtifactRequirement, ColorRequirement, DetailRequirement, OrientationRequirement,
        PresentationRequirement, SharpeningState,
    },
    decode_control::DecodePriority,
    delivery::THUMBNAIL_EDGE,
};
use oxy_domain::{PreviewResult, RenderLevel};
use oxy_runtime::CancellationToken;
use std::path::Path;

pub(crate) fn encode_cached_jpeg_thumbnail(
    result: &PreviewResult,
    priority: DecodePriority,
    cancellation: &CancellationToken,
) -> Result<EncodedJpegThumbnail, MediaError> {
    let bytes = if let Some(resource) = &result.resource {
        let registry = crate::publication::shared_resource_registry();
        let lease = registry
            .resolve(&resource.resource_id)
            .ok_or_else(|| MediaError::CacheArtifact("derivation resource expired".into()))?;
        registry.materialize(&lease)?
    } else {
        std::fs::read(&result.path)?
    };

    let mut facts = result
        .image_facts
        .clone()
        .ok_or_else(|| MediaError::CacheArtifact("cached derivation has no source facts".into()))?;
    let mut encoded = encode_jpeg_thumbnail(bytes, priority, cancellation)?;
    if !facts.is_consistent()
        || encoded.facts.source.encoded_dimensions != Some(facts.encoded_dimensions)
        || encoded.facts.source.display_dimensions != facts.display_dimensions
    {
        return Err(MediaError::CacheArtifact(
            "derivation payload disagrees with image facts".into(),
        ));
    }
    facts.processing.push(oxy_domain::ImageOperation::Decode {
        backend: "libjpeg".into(),
    });
    facts.resize(encoded.facts.display_dimensions);
    // Normalization may apply an EXIF transform, but never changes content provenance.
    facts.processing.extend(
        encoded
            .facts
            .processing
            .iter()
            .filter(|step| {
                matches!(
                    step,
                    oxy_domain::ImageOperation::Orient { .. }
                        | oxy_domain::ImageOperation::Encode { .. }
                )
            })
            .cloned(),
    );
    facts.encoded_dimensions = encoded.facts.encoded_dimensions;
    facts.exif_orientation = 1;
    facts.byte_integrity = oxy_domain::ByteIntegrity::Reencoded;
    encoded.facts = facts;
    Ok(encoded)
}

/// Preserve the qualified HEIF 160-pixel path; only normalize a larger cache
/// substitution before it crosses the browser thumbnail delivery boundary.
pub(crate) fn ensure_thumbnail_delivery(
    path: &Path,
    cache_dir: &Path,
    result: PreviewResult,
    priority: DecodePriority,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    if result.width <= THUMBNAIL_EDGE && result.height <= THUMBNAIL_EDGE {
        return Ok(result);
    }
    let artifacts = ArtifactCache::new(path, cache_dir)?;
    let target = crate::policy::COMPATIBLE_THUMBNAIL;
    let request = artifacts.request(
        DetailRequirement::Display { min_long_edge: 1 },
        ArtifactRequirement::BoundedThumbnail { target },
        PresentationRequirement {
            orientation: OrientationRequirement::DisplayCorrect,
            color: ColorRequirement::Any,
            sharpening: SharpeningState::None,
        },
        false,
    );
    artifacts.coordinate_work(
        &request,
        RenderLevel::Thumbnail,
        target,
        || cancellation.is_cancelled(),
        |generation| {
            let EncodedJpegThumbnail {
                facts,
                bytes,
                dimensions,
                mut presentation,
            } = encode_cached_jpeg_thumbnail(&result, priority, cancellation)?;
            // Keep the existing cache identity for compatible derivations.
            presentation.color = crate::cache::CacheColorState::EmbeddedOrUnknown;
            presentation.geometry = result.geometry.map(|geometry| {
                let rect = geometry.content_rect;
                let scale_x = |x: u32| {
                    (u64::from(x) * u64::from(dimensions.width) / u64::from(result.width)) as u32
                };
                let scale_y = |y: u32| {
                    (u64::from(y) * u64::from(dimensions.height) / u64::from(result.height)) as u32
                };
                oxy_domain::PreviewGeometry {
                    display_size: geometry.display_size,
                    content_rect: oxy_domain::PreviewContentRect {
                        x: scale_x(rect.x),
                        y: scale_y(rect.y),
                        width: scale_x(rect.width).max(1),
                        height: scale_y(rect.height).max(1),
                    },
                }
            });
            artifacts.publish(
                bytes,
                facts,
                presentation,
                target.into(),
                RenderLevel::Thumbnail,
                generation,
                &request,
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_thumbnail_retains_raw_source_and_processing_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("developed.jpg");
        image::DynamicImage::new_rgb8(1200, 800)
            .save(&path)
            .unwrap();
        let mut result = crate::media_source::preview_result(
            path,
            oxy_domain::PreviewKind::Developed,
            RenderLevel::Full,
        )
        .unwrap();
        let facts = result.image_facts.as_mut().unwrap();
        facts.source.origin = oxy_domain::ImageOrigin::RawSensor;
        facts.source.candidate_id = "raw-sensor".into();
        facts.byte_integrity = oxy_domain::ByteIntegrity::Reencoded;
        facts.processing.push(oxy_domain::ImageOperation::Develop {
            backend: "LibRaw".into(),
        });
        let expected_source = facts.source.clone();
        let encoded = encode_cached_jpeg_thumbnail(
            &result,
            DecodePriority::Foreground,
            &CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(encoded.facts.source, expected_source);
        assert_eq!(
            encoded.facts.detail.reference_dimensions,
            oxy_domain::DisplayDimensions((1200, 800).into())
        );
        assert!(matches!(
            encoded.facts.processing.first(),
            Some(oxy_domain::ImageOperation::Develop { .. })
        ));
        assert_eq!(
            encoded.facts.byte_integrity,
            oxy_domain::ByteIntegrity::Reencoded
        );
        assert!(!encoded.facts.native_detail());
        assert!(encoded.facts.is_consistent());
    }
}
