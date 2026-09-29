use super::input::{MediaPixels, check_cancelled};
use crate::{CacheColorState, MediaError, SourceRevision};
use image::{DynamicImage, metadata::Orientation};
use oxy_domain::{
    ArtifactFacts, DisplayDimensions, EncodedDimensions, ImageOperation, PixelDimensions,
};
use oxy_runtime::CancellationToken;

pub(super) fn orient(
    image: &mut DynamicImage,
    facts: &mut ArtifactFacts,
    orientation: Orientation,
) {
    image.apply_orientation(orientation);
    if orientation != Orientation::NoTransforms {
        facts.processing.push(ImageOperation::Orient {
            exif: orientation.to_exif(),
        });
    }
}

pub(super) fn fit(image: DynamicImage, target: u32) -> DynamicImage {
    if image.width().max(image.height()) > target {
        image.resize(target, target, image::imageops::FilterType::Triangle)
    } else {
        image
    }
}

pub(super) fn finish(
    image: DynamicImage,
    mut facts: ArtifactFacts,
    revision: &SourceRevision,
    color: CacheColorState,
    icc_profile: Option<Vec<u8>>,
    target: u32,
    cancellation: &CancellationToken,
) -> Result<MediaPixels, MediaError> {
    check_cancelled(cancellation)?;
    let image = fit(image, target);
    facts.resize(DisplayDimensions((image.width(), image.height()).into()));
    facts.encoded_dimensions = EncodedDimensions(facts.display_dimensions.0);
    facts.exif_orientation = 1;
    check_cancelled(cancellation)?;
    Ok(MediaPixels {
        source_revision: revision.clone(),
        image,
        facts,
        color,
        icc_profile,
        cache_hit: false,
    })
}

pub(super) fn pixel_bytes(
    dimensions: PixelDimensions,
    bytes_per_pixel: u64,
) -> Result<u64, MediaError> {
    u64::from(dimensions.width)
        .checked_mul(u64::from(dimensions.height))
        .and_then(|n| n.checked_mul(bytes_per_pixel))
        .ok_or(MediaError::InvalidMediaRequest)
}

pub(super) fn estimate(
    dimensions: PixelDimensions,
    bytes_per_pixel: u64,
    encoded_bytes: u64,
) -> Result<usize, MediaError> {
    pixel_bytes(dimensions, bytes_per_pixel)?
        .checked_add(encoded_bytes)
        .and_then(|n| n.checked_add(16 * 1024 * 1024))
        .and_then(|n| usize::try_from(n).ok())
        .ok_or(MediaError::InvalidMediaRequest)
}
