mod pixels;
pub(crate) use pixels::decode_pixels;
pub(super) use pixels::{decode_bytes, decode_with_limit};
// Native raster delivery.
use super::artifact::{ArtifactCache, ArtifactEncoding, register_original_resource};
use crate::{
    MediaError,
    cache::{
        ArtifactPresentation, ArtifactRequirement, CacheColorState, ColorRequirement,
        DetailRequirement, OrientationRequirement, OrientationState, PresentationRequirement,
        SharpeningState,
    },
    decode_control::{self, DecodePriority},
    delivery::{Delivery, THUMBNAIL_EDGE, THUMBNAIL_LIMITS},
    policy::RASTER_THUMBNAIL,
};
use image::{ImageEncoder, codecs::png::PngEncoder};
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
        ArtifactRequirement::BoundedThumbnail {
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
            let frame = pixels::decode_with_limit(
                path,
                &crate::SourceRevision::observe(path)?,
                THUMBNAIL_EDGE,
                cost,
                cancellation,
            )?;
            let output = frame.facts.display_dimensions.0;
            let image = frame.image.to_rgba8();
            let profile = frame.icc_profile;
            let mut facts = frame.facts;
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
            facts.processing.push(oxy_domain::ImageOperation::Encode {
                format: "png".into(),
            });
            facts.encoded_dimensions = oxy_domain::EncodedDimensions(output);
            facts.exif_orientation = 1;
            facts.byte_integrity = oxy_domain::ByteIntegrity::Reencoded;
            artifacts.publish_encoded(
                bytes.into(),
                facts.clone(),
                ArtifactPresentation {
                    geometry: None,
                    orientation: OrientationState::Applied,
                    color: CacheColorState::EmbeddedOrUnknown,
                    sharpening: SharpeningState::None,
                },
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
