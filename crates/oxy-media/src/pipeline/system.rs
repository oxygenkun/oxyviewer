#[cfg(target_os = "macos")]
use crate::backends::apple_image_io;
use crate::{
    MediaError,
    cache::{
        ArtifactPresentation, ArtifactRepresentation, CacheColorState, DetailRequirement,
        DisplayDimensions, OrientationRequirement, OrientationState, PresentationRequirement,
        RepresentationRequirement, SharpeningState,
    },
    media_source::has_complete_jpeg_markers,
    pipeline::artifact::ArtifactCache,
    policy::SYSTEM_PREVIEW,
};
use oxy_domain::{PreviewResult, RenderLevel};
use std::path::Path;

pub fn preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    level: RenderLevel,
    allow_interim: bool,
) -> Result<PreviewResult, MediaError> {
    let artifacts = ArtifactCache::new(path, cache_dir)?;
    let presentation = ArtifactPresentation {
        geometry: None,
        orientation: OrientationState::Applied,
        color: CacheColorState::EmbeddedOrUnknown,
        sharpening: SharpeningState::None,
    };
    let detail = if level == RenderLevel::Full {
        DetailRequirement::NativeDetail
    } else {
        DetailRequirement::Display {
            min_long_edge: max_size,
        }
    };
    let request = artifacts.request(
        detail,
        RepresentationRequirement::Exact(ArtifactRepresentation::System),
        PresentationRequirement {
            orientation: OrientationRequirement::Exact(presentation.orientation),
            color: crate::cache::ColorRequirement::Any,
            sharpening: presentation.sharpening,
        },
        allow_interim,
    );
    let production_request = artifacts.request(
        request.detail,
        request.representation,
        request.presentation,
        allow_interim,
    );
    if let Some(result) = artifacts.lookup(&request, level)? {
        return Ok(result);
    }
    artifacts.coordinate_work(
        &request,
        level,
        "system-compatible-development",
        || false,
        |generation| {
            let temporary = artifacts.temporary_output(".jpg")?;
            generate(path, &temporary, max_size)?;
            if !has_complete_jpeg_markers(&temporary)? {
                return Err(MediaError::PreviewGenerationFailed {
                    path: path.to_owned(),
                    message: "native preview produced a truncated JPEG".into(),
                });
            }
            let (width, height) = image::image_dimensions(&temporary)?;
            let source_dimensions = match image::image_dimensions(path) {
                Ok(dimensions) => dimensions,
                Err(_error) if is_heif_path(path) => {
                    let dimensions = crate::backends::libheif::dimensions(path)?;
                    (dimensions.width, dimensions.height)
                }
                Err(error) => return Err(error.into()),
            };
            let native_detail =
                sorted_dimensions((width, height)) == sorted_dimensions(source_dimensions);
            artifacts.publish_staged(
                temporary,
                DisplayDimensions { width, height },
                ArtifactRepresentation::System,
                presentation,
                native_detail,
                format!("{SYSTEM_PREVIEW}:{max_size}"),
                level,
                generation,
                &production_request,
            )
        },
    )
}

fn is_heif_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "heif" | "heic" | "hif"
    )
}

fn sorted_dimensions((width, height): (u32, u32)) -> (u32, u32) {
    (width.min(height), width.max(height))
}

#[cfg(target_os = "macos")]
fn generate(source: &Path, destination: &Path, max_size: u32) -> Result<(), MediaError> {
    apple_image_io::render_jpeg(source, destination, Some(max_size), 90)
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn generate(_source: &Path, _destination: &Path, _max_size: u32) -> Result<(), MediaError> {
    Err(MediaError::NativeDecoderUnavailable)
}
