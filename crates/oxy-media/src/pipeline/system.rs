//! System image decoding with a portable raster fallback, shared by both deliveries.
use crate::{
    MediaError, MediaRequest, SourceRevision,
    cache::{
        ArtifactPresentation, ArtifactRequirement, DetailRequirement, OrientationState,
        SharpeningState,
    },
    decode_control::{self, DecodePriority},
    pipeline::{
        artifact::{ArtifactCache, ArtifactEncoding},
        input::MediaPixels,
        pixels,
    },
    policy::SYSTEM_PREVIEW,
};
use image::{ImageEncoder, codecs::png::PngEncoder};
use oxy_domain::{PreviewResult, RenderLevel};
use oxy_runtime::CancellationToken;
use std::path::Path;

pub(crate) fn preview(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    allow_interim: bool,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let max_size = if level == RenderLevel::Full {
        let size = source_dimensions(path)?;
        size.width.max(size.height)
    } else {
        512
    };
    preview_with_size(
        path,
        cache_dir,
        max_size,
        level,
        allow_interim,
        cancellation,
    )
}

pub(crate) fn preview_with_size(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    level: RenderLevel,
    allow_interim: bool,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let artifacts = ArtifactCache::new(path, cache_dir)?;
    let request = artifacts.request(
        if level == RenderLevel::Full {
            DetailRequirement::NativeDetail
        } else {
            DetailRequirement::Display {
                min_long_edge: max_size,
            }
        },
        ArtifactRequirement::AnyDisplay,
        MediaRequest::unsharpened(),
        allow_interim,
    );
    artifacts.coordinate_work(
        &request,
        level,
        "system-compatible-development",
        || cancellation.is_cancelled(),
        |generation| {
            let source = SourceRevision::observe(path)?;
            let cost = pixels::estimate(source_dimensions(path)?, 16, source.size_bytes)?;
            let _permit =
                decode_control::acquire_conversion(DecodePriority::Foreground, cost, &|| {
                    cancellation.is_cancelled()
                })?;
            let frame = decode_with_limit(path, &source, max_size, cost, cancellation)?;
            let rgba = frame.image.to_rgba8();
            let mut bytes = Vec::new();
            let mut encoder = PngEncoder::new(&mut bytes);
            if let Some(profile) = frame.icc_profile {
                encoder
                    .set_icc_profile(profile)
                    .map_err(|error| MediaError::Color(error.to_string()))?;
            }
            encoder.write_image(
                &rgba,
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
            let mut facts = frame.facts;
            facts.processing.push(oxy_domain::ImageOperation::Encode {
                format: "png".into(),
            });
            facts.byte_integrity = oxy_domain::ByteIntegrity::Reencoded;
            artifacts.publish_encoded(
                bytes.into(),
                facts,
                ArtifactPresentation {
                    geometry: None,
                    orientation: OrientationState::Applied,
                    color: frame.color,
                    sharpening: SharpeningState::None,
                },
                format!("{SYSTEM_PREVIEW}:{max_size}"),
                level,
                generation,
                &request,
                ArtifactEncoding::Png,
            )
        },
    )
}

fn source_dimensions(path: &Path) -> Result<oxy_domain::PixelDimensions, MediaError> {
    image::image_dimensions(path)
        .map(Into::into)
        .or_else(|_| crate::backends::libheif::dimensions(path))
}

pub(crate) fn decode_pixels(
    path: &Path,
    source: &SourceRevision,
    target: u32,
    cancellation: &CancellationToken,
) -> Result<MediaPixels, MediaError> {
    let cost = pixels::estimate(source_dimensions(path)?, 16, source.size_bytes)?;
    let _permit = decode_control::acquire_pixel_input(cost, &|| cancellation.is_cancelled())?;
    decode_with_limit(path, source, target, cost, cancellation)
}

fn decode_with_limit(
    path: &Path,
    source: &SourceRevision,
    target: u32,
    cost: usize,
    cancellation: &CancellationToken,
) -> Result<MediaPixels, MediaError> {
    super::input::check_cancelled(cancellation)?;
    #[cfg(target_os = "macos")]
    if let Ok(image) = crate::backends::apple_image_io::decode_rgba8(path, target) {
        let encoded = oxy_domain::EncodedDimensions(source_dimensions(path)?);
        use image::ImageDecoder;
        let orientation = image::ImageReader::open(path)
            .ok()
            .and_then(|reader| reader.with_guessed_format().ok())
            .and_then(|reader| reader.into_decoder().ok())
            .and_then(|mut decoder| decoder.orientation().ok())
            .map_or(1, image::metadata::Orientation::to_exif);
        let reference = encoded.to_display(orientation);
        let facts = crate::media_source::decoded_facts(
            oxy_domain::ImageOrigin::PrimaryImage,
            "primary".into(),
            reference,
            reference,
            oxy_domain::DisplayDimensions((image.width(), image.height()).into()),
            "Apple ImageIO",
        );
        return pixels::finish(
            image,
            facts,
            source,
            crate::CacheColorState::Srgb,
            None,
            target,
            cancellation,
        );
    }
    super::input::check_cancelled(cancellation)?;
    super::raster::decode_with_limit(path, source, target, cost, cancellation)
}
