//! Shared JPEG thumbnail encoding for camera previews and cached representations.
use crate::{
    MediaError,
    backends::libjpeg::{self, DecodePlan},
    cache::{
        ArtifactPresentation, CacheColorState, DisplayDimensions, OrientationState, SharpeningState,
    },
    decode_control::{self, DecodePriority},
    delivery::{Delivery, THUMBNAIL_EDGE, THUMBNAIL_LIMITS},
    formats::jpeg::{self, Header},
};
use image::{
    ImageDecoder, ImageEncoder,
    codecs::jpeg::{JpegDecoder, JpegEncoder},
    metadata::Orientation,
};
use oxy_runtime::CancellationToken;
use std::{io::Cursor, sync::Arc};

/// Encoded pixels and their presentation travel together to the publication boundary.
pub(crate) struct EncodedJpegThumbnail {
    pub bytes: Arc<[u8]>,
    pub dimensions: DisplayDimensions,
    pub presentation: ArtifactPresentation,
}

pub(super) struct ConversionPlan {
    pub cost: usize,
    pub decode: DecodePlan,
}

pub(super) fn plan_conversion(
    dimensions: DisplayDimensions,
    bytes: u64,
) -> Result<ConversionPlan, MediaError> {
    // Worst-case full 4:4:4 progressive coefficients (6 bytes/pixel),
    // IDCT-reduced RGB and resampling buffers, two encoded inputs, and scratch.
    // This is temporary conversion admission, not total process memory.
    let decode = libjpeg::plan_decode(dimensions, THUMBNAIL_EDGE);
    let scaled = decode.output;
    let scaled_bytes = u64::from(scaled.width) * u64::from(scaled.height) * 6;
    let cost = u64::from(dimensions.width)
        .checked_mul(u64::from(dimensions.height))
        .and_then(|pixels| pixels.checked_mul(6))
        .and_then(|pixels| {
            bytes
                .checked_mul(2)
                .and_then(|bytes| pixels.checked_add(bytes))
        })
        .and_then(|bytes| bytes.checked_add(scaled_bytes))
        .and_then(|bytes| bytes.checked_add(16 * 1024 * 1024))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| {
            MediaError::CacheArtifact("thumbnail conversion estimate overflow".into())
        })?;
    Ok(ConversionPlan { cost, decode })
}

pub(crate) fn encode_jpeg_thumbnail(
    bytes: Vec<u8>,
    priority: DecodePriority,
    cancellation: &CancellationToken,
) -> Result<EncodedJpegThumbnail, MediaError> {
    let header = jpeg::probe(&mut Cursor::new(&bytes), bytes.len() as u64, || {
        cancellation.is_cancelled()
    })?;
    let dimensions = header
        .dimensions
        .ok_or_else(|| MediaError::CacheArtifact("camera JPEG has no dimensions".into()))?;
    let conversion = plan_conversion(dimensions, bytes.len() as u64)?;
    let _permit = decode_control::acquire_conversion(priority, conversion.cost, &|| {
        cancellation.is_cancelled()
    })?;
    normalize(bytes, &header, None, conversion.decode, cancellation)
}

pub(super) fn presentation(srgb: bool) -> ArtifactPresentation {
    ArtifactPresentation {
        geometry: None,
        orientation: OrientationState::Applied,
        color: if srgb {
            CacheColorState::Srgb
        } else {
            CacheColorState::EmbeddedOrUnknown
        },
        sharpening: SharpeningState::None,
    }
}

pub(super) fn normalize(
    bytes: Vec<u8>,
    primary: &Header,
    embedded: Option<&Header>,
    plan: DecodePlan,
    cancellation: &CancellationToken,
) -> Result<EncodedJpegThumbnail, MediaError> {
    let mut decoder = JpegDecoder::new(Cursor::new(&bytes))?;
    let mut profile = decoder.icc_profile()?;
    if embedded.unwrap_or(primary).has_icc && profile.is_none() {
        return Err(MediaError::Color("incomplete JPEG ICC profile".into()));
    }
    if let Some(profile) = &profile {
        let profile = lcms2::Profile::new_icc(profile)
            .map_err(|error| MediaError::Color(error.to_string()))?;
        if profile.color_space() != lcms2::ColorSpaceSignature::RgbData {
            return Err(MediaError::Color(
                "JPEG thumbnail needs an RGB ICC profile".into(),
            ));
        }
    }
    let orientation = if embedded.is_some() {
        Orientation::from_exif(primary.exif.orientation.unwrap_or(1) as u8)
            .unwrap_or(Orientation::NoTransforms)
    } else {
        decoder.orientation()?
    };
    if profile.is_none() && primary.exif.srgb && !primary.has_icc {
        profile = Some(
            lcms2::Profile::new_srgb()
                .icc()
                .map_err(|error| MediaError::Color(error.to_string()))?,
        );
    }
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    drop(decoder);
    let image = crate::backends::libjpeg::decode_scaled(&bytes, plan, cancellation)?;
    // Resize before orientation avoids a second full-size rotation allocation.
    let target = THUMBNAIL_EDGE.min(image.width().max(image.height()));
    let mut image = image.resize(target, target, image::imageops::FilterType::Triangle);
    image.apply_orientation(orientation);
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let dimensions = DisplayDimensions {
        width: image.width(),
        height: image.height(),
    };
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 90);
    if let Some(profile) = profile {
        encoder
            .set_icc_profile(profile)
            .map_err(|error| MediaError::Color(error.to_string()))?;
    }
    encoder.encode_image(&image)?;
    if THUMBNAIL_LIMITS.classify(dimensions, bytes.len() as u64) != Delivery::Direct {
        return Err(MediaError::CacheArtifact(
            "normalized JPEG exceeds thumbnail delivery budget".into(),
        ));
    }
    Ok(EncodedJpegThumbnail {
        bytes: bytes.into(),
        dimensions,
        presentation: presentation(
            primary.exif.srgb && !primary.has_icc && !embedded.is_some_and(|header| header.has_icc),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::DynamicImage;
    #[test]
    fn incomplete_or_invalid_embedded_icc_never_inherits_srgb() {
        let mut encoded = Vec::new();
        JpegEncoder::new(&mut encoded)
            .encode_image(&DynamicImage::new_rgb8(800, 600))
            .unwrap();
        let primary = Header {
            exif: oxy_metadata_parser::jpeg_preview::ExifPresentation {
                srgb: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let child = Header {
            has_icc: true,
            ..Default::default()
        };
        assert!(matches!(
            normalize(
                encoded,
                &primary,
                Some(&child),
                libjpeg::plan_decode(
                    DisplayDimensions {
                        width: 800,
                        height: 600
                    },
                    THUMBNAIL_EDGE
                ),
                &CancellationToken::default()
            ),
            Err(MediaError::Color(_))
        ));
    }

    #[test]
    fn all_exif_orientations_preserve_corner_pixels_and_profile() {
        let mut source = image::RgbImage::new(100, 60);
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            *pixel = image::Rgb(if y < 30 {
                if x < 50 { [230, 10, 10] } else { [10, 230, 10] }
            } else if x < 50 {
                [10, 10, 230]
            } else {
                [230, 230, 10]
            });
        }
        let original = DynamicImage::ImageRgb8(source);
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, 95)
            .encode_image(&original)
            .unwrap();
        for orientation in 1..=8 {
            let header = Header {
                exif: oxy_metadata_parser::jpeg_preview::ExifPresentation {
                    orientation: Some(orientation),
                    srgb: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            let EncodedJpegThumbnail {
                bytes, dimensions, ..
            } = normalize(
                encoded.clone(),
                &header,
                Some(&header),
                libjpeg::plan_decode(
                    DisplayDimensions {
                        width: 100,
                        height: 60,
                    },
                    THUMBNAIL_EDGE,
                ),
                &CancellationToken::default(),
            )
            .unwrap();
            let mut expected = original.clone();
            expected.apply_orientation(Orientation::from_exif(orientation as u8).unwrap());
            let actual = image::load_from_memory(&bytes).unwrap().to_rgb8();
            assert_eq!(
                (dimensions.width, dimensions.height),
                (expected.width(), expected.height())
            );
            let expected = expected.to_rgb8();
            for (x, y) in [
                (10, 10),
                (10, actual.height() - 11),
                (actual.width() - 11, 10),
                (actual.width() - 11, actual.height() - 11),
            ] {
                for (actual, expected) in actual
                    .get_pixel(x, y)
                    .0
                    .into_iter()
                    .zip(expected.get_pixel(x, y).0)
                {
                    assert!((i16::from(actual) - i16::from(expected)).abs() < 15);
                }
            }
            assert!(
                JpegDecoder::new(Cursor::new(bytes))
                    .unwrap()
                    .icc_profile()
                    .unwrap()
                    .is_some()
            );
        }
    }
}
