//! Native PNG/WebP thumbnail conversion. Preserve alpha and ICC in a small PNG;
//! the browser receives an already bounded image, never a full raster to resize.
use super::artifact::{ArtifactCache, ArtifactEncoding, register_original_resource};
use crate::{
    MediaError,
    cache::{
        ArtifactPresentation, ArtifactRepresentation, CacheColorState, ColorRequirement,
        DetailRequirement, DisplayDimensions, OrientationRequirement, OrientationState,
        PresentationRequirement, RepresentationRequirement, SharpeningState,
    },
    decode_control::{self, DecodePriority},
    delivery::{Delivery, THUMBNAIL_EDGE, THUMBNAIL_LIMITS},
    policy::RASTER_THUMBNAIL,
};
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, codecs::png::PngEncoder};
use oxy_domain::{PreviewKind, PreviewResult, RenderLevel};
use oxy_runtime::CancellationToken;
use std::path::Path;

pub(crate) fn thumbnail(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    priority: DecodePriority,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let artifacts = ArtifactCache::new(path, cache_dir)?;
    let request = artifacts.request(
        DetailRequirement::Display {
            min_long_edge: THUMBNAIL_EDGE,
        },
        RepresentationRequirement::BoundedThumbnail {
            target: RASTER_THUMBNAIL,
        },
        PresentationRequirement {
            orientation: OrientationRequirement::DisplayCorrect,
            color: ColorRequirement::Any,
            sharpening: SharpeningState::None,
        },
        false,
    );
    artifacts.coordinate_work(
        &request,
        level,
        RASTER_THUMBNAIL,
        || cancellation.is_cancelled(),
        |generation| {
            let length = std::fs::metadata(path)?.len();
            let dimensions = image::image_dimensions(path)?.into();
            if THUMBNAIL_LIMITS.classify(dimensions, length) == Delivery::Direct {
                return register_original_resource(
                    path,
                    crate::media_source::preview_result(
                        path.to_owned(),
                        PreviewKind::Original,
                        level,
                    )?,
                );
            }
            // RGBA16 input plus decompressor/resampler working space. Reserve before
            // constructing the pixel decoder; metadata reads above are header-only.
            let pixels = u64::from(dimensions.width) * u64::from(dimensions.height);
            let cost = pixels
                .checked_mul(16)
                .and_then(|value| value.checked_add(length))
                .and_then(|value| value.checked_add(8 * 1024 * 1024))
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    MediaError::CacheArtifact("raster thumbnail budget overflow".into())
                })?;
            let _permit = decode_control::acquire_conversion(priority, cost, &|| {
                cancellation.is_cancelled()
            })?;
            let mut reader = ImageReader::open(path)?.with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(cost as u64);
            reader.limits(limits);
            let mut decoder = reader.into_decoder()?;
            let orientation = decoder.orientation()?;
            let profile = decoder.icc_profile()?;
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            let image = DynamicImage::from_decoder(decoder)?;
            let target = THUMBNAIL_EDGE.min(image.width().max(image.height()));
            let mut image = image.resize(target, target, image::imageops::FilterType::Triangle);
            image.apply_orientation(orientation);
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            let output = DisplayDimensions {
                width: image.width(),
                height: image.height(),
            };
            let image = image.to_rgba8();
            let mut bytes = Vec::new();
            let mut encoder = PngEncoder::new(&mut bytes);
            if let Some(profile) = profile {
                encoder
                    .set_icc_profile(profile)
                    .map_err(|error| MediaError::Color(error.to_string()))?;
            }
            encoder.write_image(
                &image,
                output.width,
                output.height,
                image::ExtendedColorType::Rgba8,
            )?;
            artifacts.publish_encoded(
                bytes.into(),
                output,
                ArtifactRepresentation::Decoded,
                ArtifactPresentation {
                    geometry: None,
                    orientation: OrientationState::Applied,
                    color: CacheColorState::EmbeddedOrUnknown,
                    sharpening: SharpeningState::None,
                },
                dimensions.width.max(dimensions.height) <= THUMBNAIL_EDGE,
                RASTER_THUMBNAIL.into(),
                level,
                generation,
                &request,
                ArtifactEncoding::Png,
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_and_webp_keep_alpha_and_full_source() {
        let directory = tempfile::tempdir().unwrap();
        for extension in ["png", "webp"] {
            let path = directory.path().join(format!("alpha.{extension}"));
            let image = image::RgbaImage::from_pixel(1200, 800, image::Rgba([30, 100, 200, 60]));
            image.save(&path).unwrap();
            let result = thumbnail(
                &path,
                directory.path(),
                RenderLevel::Thumbnail,
                DecodePriority::Visible,
                &CancellationToken::default(),
            )
            .unwrap();
            assert_eq!((result.width, result.height), (512, 341));
            assert_ne!(result.path, path);
            let pixel = image::open(result.path)
                .unwrap()
                .to_rgba8()
                .get_pixel(250, 150)
                .0;
            assert_eq!(pixel, [30, 100, 200, 60]);
            assert_eq!(image::image_dimensions(&path).unwrap(), (1200, 800));
        }
    }
}
