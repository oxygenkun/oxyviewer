#[cfg(target_os = "macos")]
use crate::backends::apple_image_io;
use crate::{
    MediaError,
    cache::{
        ArtifactPresentation, ArtifactRequirement, CacheColorState, DetailRequirement, ImageOrigin,
        OrientationRequirement, OrientationState, PresentationRequirement, SharpeningState,
    },
    media_source::has_complete_jpeg_markers,
    pipeline::artifact::ArtifactCache,
    policy::SYSTEM_PREVIEW,
};
use oxy_domain::{PreviewResult, RenderLevel};
use std::path::Path;

pub(crate) fn preview(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    allow_interim: bool,
    cancellation: &oxy_runtime::CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let max_size = match level {
        RenderLevel::Thumbnail | RenderLevel::Preview => 512,
        RenderLevel::Full => 4_096,
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
    cancellation: &oxy_runtime::CancellationToken,
) -> Result<PreviewResult, MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
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
        ArtifactRequirement::Exact(ImageOrigin::PrimaryImage),
        PresentationRequirement {
            orientation: OrientationRequirement::Exact(presentation.orientation),
            color: crate::cache::ColorRequirement::Any,
            sharpening: presentation.sharpening,
        },
        allow_interim,
    );
    let production_request = artifacts.request(
        request.detail,
        request.artifact.clone(),
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
        || cancellation.is_cancelled(),
        |generation| {
            let temporary = artifacts.temporary_output(".jpg")?;
            generate(path, &temporary, max_size)?;
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
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
            use image::ImageDecoder;
            let orientation = image::ImageReader::open(path)
                .ok()
                .and_then(|reader| reader.with_guessed_format().ok())
                .and_then(|reader| reader.into_decoder().ok())
                .and_then(|mut decoder| decoder.orientation().ok())
                .map_or(1, image::metadata::Orientation::to_exif);
            let encoded = oxy_domain::EncodedDimensions(source_dimensions.into());
            let reference = encoded.to_display(orientation);
            let mut facts = crate::media_source::source_facts(
                ImageOrigin::PrimaryImage,
                "primary".into(),
                encoded,
                orientation,
                reference,
            );
            facts.processing.push(oxy_domain::ImageOperation::Decode {
                backend: "system:ImageIO".into(),
            });
            facts.resize(oxy_domain::DisplayDimensions((width, height).into()));
            if orientation != 1 {
                facts
                    .processing
                    .push(oxy_domain::ImageOperation::Orient { exif: orientation });
            }
            facts.processing.push(oxy_domain::ImageOperation::Encode {
                format: "jpeg".into(),
            });
            facts.encoded_dimensions = oxy_domain::EncodedDimensions((width, height).into());
            facts.exif_orientation = 1;
            facts.byte_integrity = oxy_domain::ByteIntegrity::Reencoded;
            artifacts.publish_staged(
                temporary,
                facts.clone(),
                presentation,
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

#[cfg(target_os = "macos")]
fn generate(source: &Path, destination: &Path, max_size: u32) -> Result<(), MediaError> {
    apple_image_io::render_jpeg(source, destination, Some(max_size), 90)
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn generate(_source: &Path, _destination: &Path, _max_size: u32) -> Result<(), MediaError> {
    Err(MediaError::NativeDecoderUnavailable)
}
