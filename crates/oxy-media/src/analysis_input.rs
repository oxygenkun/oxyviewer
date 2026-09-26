//! Bounded native pixels for explicit background person analysis. Display
//! thumbnails do not meet this input contract merely because they are cached.

use crate::{
    MediaError,
    backends::libjpeg,
    cache::{PixelDimensions, SourceRevision},
    decode_control::{self, DecodePriority},
};
use image::{ImageDecoder, RgbImage, codecs::jpeg::JpegDecoder};
use oxy_domain::AssetKind;
use oxy_runtime::CancellationToken;
use std::{fs::File, io::BufReader, path::Path};

#[derive(Debug, Clone, Copy)]
pub struct AnalysisRequirement {
    /// Minimum long edge of the encoded source. Below this is unknown, not empty.
    pub minimum_source_long_edge: u32,
    /// Maximum long edge returned to the model adapter after bounded decode.
    pub target_long_edge: u32,
}

#[derive(Debug)]
pub struct AnalysisInput {
    pub source_revision: SourceRevision,
    pub source_dimensions: PixelDimensions,
    pub pixels: RgbImage,
    /// The source had an ICC profile; the current JPEG path leaves channel
    /// values in its encoded RGB space for model-specific preprocessing.
    pub icc_present: bool,
}

/// Prepare one verified, oriented JPEG frame off the UI thread. The caller
/// retains it only for the duration of one asset's detection/encoding stages.
/// Other formats need their own qualified representation planners.
pub fn prepare_analysis_input(
    path: &Path,
    kind: AssetKind,
    expected: &SourceRevision,
    requirement: AnalysisRequirement,
    cancellation: &CancellationToken,
) -> Result<AnalysisInput, MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    if kind != AssetKind::Jpeg {
        return Err(MediaError::AnalysisFormatUnavailable);
    }
    if requirement.minimum_source_long_edge == 0 || requirement.target_long_edge == 0 {
        return Err(MediaError::CacheArtifact(
            "invalid analysis input requirement".into(),
        ));
    }
    let observed = SourceRevision::observe(path)?;
    if observed != *expected {
        return Err(MediaError::StaleSourceRevision);
    }
    let mut decoder = JpegDecoder::new(BufReader::new(File::open(path)?))?;
    let (width, height) = decoder.dimensions();
    let source_dimensions = PixelDimensions { width, height };
    let source_edge = width.max(height);
    if source_edge < requirement.minimum_source_long_edge {
        return Err(MediaError::InsufficientAnalysisDetail {
            available: source_edge,
            required: requirement.minimum_source_long_edge,
        });
    }
    let orientation = decoder.orientation()?;
    let icc_present = decoder.icc_profile()?.is_some();
    drop(decoder);

    let plan = libjpeg::plan_decode(
        oxy_domain::EncodedDimensions(source_dimensions),
        requirement.target_long_edge,
    );
    let decoded = plan.output.0;
    // Bound compressed bytes, libjpeg working space and two RGB buffers for
    // orientation/resampling before materializing the source image.
    let cost = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(6))
        .and_then(|working| {
            u64::from(decoded.width)
                .checked_mul(u64::from(decoded.height))
                .and_then(|pixels| pixels.checked_mul(6))
                .and_then(|rgb| working.checked_add(rgb))
        })
        .and_then(|bytes| bytes.checked_add(observed.size_bytes))
        .and_then(|bytes| bytes.checked_add(16 * 1024 * 1024))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| MediaError::CacheArtifact("analysis decode budget overflow".into()))?;
    let _permit = decode_control::acquire_conversion(DecodePriority::Background, cost, &|| {
        cancellation.is_cancelled()
    })?;
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() as u64 != observed.size_bytes {
        return Err(MediaError::StaleSourceRevision);
    }
    let mut pixels = libjpeg::decode_scaled(&bytes, plan, cancellation)?;
    let target = requirement
        .target_long_edge
        .min(pixels.width().max(pixels.height()));
    if target < pixels.width().max(pixels.height()) {
        pixels = pixels.resize(target, target, image::imageops::FilterType::Triangle);
    }
    pixels.apply_orientation(orientation);
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    if SourceRevision::observe(path)? != observed {
        return Err(MediaError::StaleSourceRevision);
    }
    Ok(AnalysisInput {
        source_revision: observed,
        source_dimensions,
        pixels: pixels.to_rgb8(),
        icc_present,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage, codecs::jpeg::JpegEncoder};

    #[test]
    fn jpeg_input_scales_and_rejects_stale_or_insufficient_sources() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("photo.jpg");
        let image = DynamicImage::ImageRgb8(RgbImage::new(1600, 1000));
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, 85)
            .encode_image(&image)
            .unwrap();
        std::fs::write(&path, encoded).unwrap();
        let revision = SourceRevision::observe(&path).unwrap();
        let cancellation = CancellationToken::default();
        let result = prepare_analysis_input(
            &path,
            AssetKind::Jpeg,
            &revision,
            AnalysisRequirement {
                minimum_source_long_edge: 1200,
                target_long_edge: 800,
            },
            &cancellation,
        )
        .unwrap();
        assert_eq!(result.pixels.dimensions(), (800, 500));
        assert_eq!(result.source_revision, revision);
        assert!(matches!(
            prepare_analysis_input(
                &path,
                AssetKind::Jpeg,
                &revision,
                AnalysisRequirement {
                    minimum_source_long_edge: 2000,
                    target_long_edge: 800,
                },
                &cancellation,
            ),
            Err(MediaError::InsufficientAnalysisDetail { .. })
        ));
        std::fs::write(&path, b"replacement").unwrap();
        assert!(matches!(
            prepare_analysis_input(
                &path,
                AssetKind::Jpeg,
                &revision,
                AnalysisRequirement {
                    minimum_source_long_edge: 1200,
                    target_long_edge: 800,
                },
                &cancellation,
            ),
            Err(MediaError::StaleSourceRevision)
        ));
    }

    #[test]
    fn jpeg_input_applies_exif_orientation_before_returning_pixels() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("rotated.jpg");
        let image = DynamicImage::ImageRgb8(RgbImage::new(80, 40));
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 85)
            .encode_image(&image)
            .unwrap();
        // Exif APP1 with a little-endian Orientation=6 IFD entry.
        let exif: [u8; 32] = [
            b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1,
            0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut encoded = vec![0xff, 0xd8, 0xff, 0xe1, 0, 34];
        encoded.extend_from_slice(&exif);
        encoded.extend_from_slice(&jpeg[2..]);
        std::fs::write(&path, encoded).unwrap();
        let revision = SourceRevision::observe(&path).unwrap();
        let result = prepare_analysis_input(
            &path,
            AssetKind::Jpeg,
            &revision,
            AnalysisRequirement {
                minimum_source_long_edge: 80,
                target_long_edge: 80,
            },
            &CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(
            result.source_dimensions,
            PixelDimensions {
                width: 80,
                height: 40
            }
        );
        assert_eq!(result.pixels.dimensions(), (40, 80));
    }
}
