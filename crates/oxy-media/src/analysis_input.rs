//! Model contract adapter. All media selection and preparation belong to pipeline.
#[cfg(test)]
mod tests;
use crate::pipeline::input::{MediaPixels, MediaService};
use crate::{CacheColorState, MediaError, MediaRequest, SourceRevision};
use image::RgbImage;
pub use oxy_domain::AnalysisRequirement;
use oxy_domain::{ArtifactFacts, AssetKind};
use oxy_runtime::CancellationToken;
use std::path::Path;
#[derive(Debug)]
pub struct AnalysisInput {
    pub source_revision: SourceRevision,
    /// RGB8, display-oriented, full frame, never enlarged or display-sharpened.
    /// BGR conversion, normalization and model crops belong to the consumer.
    pub pixels: RgbImage,
    /// Actual sampling, reference canvas, source and processing history.
    pub facts: ArtifactFacts,
    /// Encoded RGB is retained unless the chosen decoder establishes sRGB.
    /// This is deliberately not an unconditional color-management promise.
    pub color: CacheColorState,
    pub icc_present: bool,
    pub cache_hit: bool,
}

pub struct AnalysisInputService {
    media: MediaService,
}
impl AnalysisInputService {
    pub fn new(cache_dir: &Path) -> Result<Self, MediaError> {
        Ok(Self {
            media: MediaService::new(cache_dir)?,
        })
    }
    pub fn prepare(
        &self,
        path: &Path,
        kind: AssetKind,
        expected: &SourceRevision,
        requirement: AnalysisRequirement,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisInput, MediaError> {
        let request = request(path, kind, expected, requirement, cancellation)?;
        self.media
            .prepare_pixels(path, kind, expected, request, cancellation)
            .map(into_analysis)
    }
}
pub fn prepare_analysis_input(
    path: &Path,
    kind: AssetKind,
    expected: &SourceRevision,
    requirement: AnalysisRequirement,
    cancellation: &CancellationToken,
) -> Result<AnalysisInput, MediaError> {
    let request = request(path, kind, expected, requirement, cancellation)?;
    crate::pipeline::input::prepare_uncached(path, kind, expected, request, cancellation)
        .map(into_analysis)
}
fn request(
    path: &Path,
    kind: AssetKind,
    expected: &SourceRevision,
    requirement: AnalysisRequirement,
    cancellation: &CancellationToken,
) -> Result<MediaRequest, MediaError> {
    crate::pipeline::input::check_cancelled(cancellation)?;
    if requirement.minimum_source_long_edge == 0
        || requirement.target_long_edge < requirement.minimum_source_long_edge
    {
        return Err(MediaError::InvalidAnalysisRequirement);
    }
    if SourceRevision::observe(path)? != *expected {
        return Err(MediaError::StaleSourceRevision);
    }
    let dimensions = crate::dimensions(path, kind)?;
    let edge = dimensions.width.max(dimensions.height);
    if edge < requirement.minimum_source_long_edge {
        return Err(MediaError::InsufficientAnalysisDetail {
            available: edge,
            required: requirement.minimum_source_long_edge,
        });
    }
    Ok(MediaRequest::full_frame(
        dimensions,
        requirement.target_long_edge,
    ))
}
fn into_analysis(input: MediaPixels) -> AnalysisInput {
    let pixels = if input.image.color().has_alpha() {
        let rgba = input.image.to_rgba8();
        RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
            let pixel = rgba.get_pixel(x, y).0;
            let alpha = u32::from(pixel[3]);
            image::Rgb([0, 1, 2].map(|channel| {
                ((u32::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8
            }))
        })
    } else {
        input.image.into_rgb8()
    };
    AnalysisInput {
        source_revision: input.source_revision,
        pixels,
        facts: input.facts,
        color: input.color,
        icc_present: input.icc_profile.is_some(),
        cache_hit: input.cache_hit,
    }
}
