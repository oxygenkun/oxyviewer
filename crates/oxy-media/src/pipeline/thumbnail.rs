//! Adapt cached representations to the thumbnail delivery contract.
use super::{
    artifact::ArtifactCache,
    jpeg_transform::{EncodedJpegThumbnail, encode_jpeg_thumbnail},
};
use crate::{
    MediaError,
    cache::{
        ArtifactRepresentation, ColorRequirement, DetailRequirement, OrientationRequirement,
        PresentationRequirement, RepresentationRequirement, SharpeningState,
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
    encode_jpeg_thumbnail(bytes, priority, cancellation)
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
        RepresentationRequirement::BoundedThumbnail { target },
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
                dimensions,
                ArtifactRepresentation::Decoded,
                presentation,
                false,
                target.into(),
                RenderLevel::Thumbnail,
                generation,
                &request,
            )
        },
    )
}
