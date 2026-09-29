use crate::pipeline::{
    input::{MediaPixels, check_cancelled},
    pixels::{estimate, finish, fit, orient, pixel_bytes},
};
use crate::{CacheColorState, MediaError, SourceRevision, backends::libjpeg, decode_control};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use oxy_domain::{EncodedDimensions, ImageOperation, ImageOrigin, PixelDimensions};
use oxy_runtime::CancellationToken;
use std::path::Path;
pub(crate) fn decode_pixels(
    path: &Path,
    revision: &SourceRevision,
    target: u32,
    cancellation: &CancellationToken,
) -> Result<MediaPixels, MediaError> {
    check_cancelled(cancellation)?;
    let length = std::fs::metadata(path)?.len();
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let format = reader.format();
    let dimensions: PixelDimensions = reader.into_dimensions()?.into();
    let plan = libjpeg::plan_decode(EncodedDimensions(dimensions), target);
    let cost = if format == Some(ImageFormat::Jpeg) {
        estimate(
            dimensions,
            6,
            length
                .checked_add(pixel_bytes(plan.output.0, 6)?)
                .ok_or(MediaError::InvalidMediaRequest)?,
        )?
    } else {
        estimate(dimensions, 16, length)?
    };
    let _permit = decode_control::acquire_pixel_input(cost, &|| cancellation.is_cancelled())?;
    decode_with_limit(path, revision, target, cost, cancellation)
}

pub(crate) fn decode_with_limit(
    path: &Path,
    revision: &SourceRevision,
    target: u32,
    cost: usize,
    cancellation: &CancellationToken,
) -> Result<MediaPixels, MediaError> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    let bytes = if reader.format() == Some(ImageFormat::Jpeg) {
        Some(std::fs::read(path)?)
    } else {
        None
    };
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(cost as u64);
    reader.limits(limits);
    decode_reader(reader, bytes.as_deref(), revision, target, cancellation)
}

pub(crate) fn decode_bytes(
    bytes: &[u8],
    revision: &SourceRevision,
    target: u32,
    cancellation: &CancellationToken,
) -> Result<MediaPixels, MediaError> {
    let probe = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let dimensions: PixelDimensions = probe.into_dimensions()?.into();
    let cost = estimate(dimensions, 16, bytes.len() as u64)?;
    let _permit = decode_control::acquire_pixel_input(cost, &|| cancellation.is_cancelled())?;
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let jpeg = (reader.format() == Some(ImageFormat::Jpeg)).then_some(bytes);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(cost as u64);
    reader.limits(limits);
    decode_reader(reader, jpeg, revision, target, cancellation)
}

fn decode_reader<R: std::io::BufRead + std::io::Seek + Send>(
    reader: ImageReader<R>,
    jpeg: Option<&[u8]>,
    revision: &SourceRevision,
    target: u32,
    cancellation: &CancellationToken,
) -> Result<MediaPixels, MediaError> {
    check_cancelled(cancellation)?;
    let mut decoder = reader.into_decoder()?;
    let dimensions = EncodedDimensions(decoder.dimensions().into());
    let orientation = decoder.orientation()?;
    let profile = decoder.icc_profile()?;
    let mut facts = crate::media_source::source_facts(
        ImageOrigin::PrimaryImage,
        "primary".into(),
        dimensions,
        orientation.to_exif(),
        dimensions.to_display(orientation.to_exif()),
    );
    let image = if let Some(bytes) = jpeg {
        drop(decoder);
        libjpeg::decode_scaled(
            bytes,
            libjpeg::plan_decode(dimensions, target),
            cancellation,
        )?
    } else {
        DynamicImage::from_decoder(decoder)?
    };
    facts.processing.push(ImageOperation::Decode {
        backend: if jpeg.is_some() {
            "libjpeg-turbo"
        } else {
            "image"
        }
        .into(),
    });
    let mut image = fit(image, target);
    orient(&mut image, &mut facts, orientation);
    finish(
        image,
        facts,
        revision,
        CacheColorState::EmbeddedOrUnknown,
        profile,
        target,
        cancellation,
    )
}
