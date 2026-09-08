#[cfg(target_os = "macos")]
use crate::backends::{apple_core_image, apple_image_io};
use crate::{
    ImageDimensions, MediaError,
    backends::libraw,
    cache::write_jpeg_atomically,
    cache::{
        ArtifactPresentation, ArtifactRepresentation, CacheColorState, DetailRequirement,
        OrientationRequirement, OrientationState, PresentationRequirement,
        RepresentationRequirement, SharpeningState,
    },
    decode_control::{
        DecodePriority, acquire_decode, acquire_file_lock, acquire_raw_full_decode, file_lock,
    },
    media_source::has_complete_jpeg_markers,
    pipeline::artifact::{
        ArtifactCache, ArtifactPreparation, applied_srgb, applied_srgb_requirement,
    },
    policy::{RAW_FULL, RAW_PREVIEW},
    presentation::{CAMERA_JPEG, RAW_DEVELOPED_JPEG},
};
use image::{ImageDecoder, ImageReader, metadata::Orientation};
use oxy_domain::{PreviewResult, RenderLevel};
use oxy_runtime::CancellationToken;
use std::{
    collections::VecDeque,
    io::Cursor,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};

const FAILED_EMBEDDED_CAPACITY: usize = 256;
static FAILED_EMBEDDED_REVISIONS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawBackend {
    AppleCoreImage,
    AppleImageIo,
    LibRawDevelopment,
}

pub(crate) fn backend_plan(level: RenderLevel) -> Vec<RawBackend> {
    #[cfg(target_os = "macos")]
    return match level {
        RenderLevel::Thumbnail => vec![
            RawBackend::AppleImageIo,
            RawBackend::AppleCoreImage,
            RawBackend::LibRawDevelopment,
        ],
        RenderLevel::Preview | RenderLevel::Full => vec![
            RawBackend::AppleCoreImage,
            RawBackend::AppleImageIo,
            RawBackend::LibRawDevelopment,
        ],
    };
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        let _ = level;
        vec![RawBackend::LibRawDevelopment]
    }
}

pub(crate) fn dimensions(path: &Path) -> Result<ImageDimensions, MediaError> {
    libraw::dimensions(path).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    })
}

fn display_requirement() -> PresentationRequirement {
    PresentationRequirement {
        orientation: OrientationRequirement::DisplayCorrect,
        color: crate::cache::ColorRequirement::Any,
        sharpening: SharpeningState::None,
    }
}

pub(crate) fn preview_request(
    artifacts: &ArtifactCache,
    max_size: u32,
    allow_interim: bool,
) -> crate::cache::CacheRequest {
    artifacts.request(
        DetailRequirement::Display {
            min_long_edge: max_size,
        },
        RepresentationRequirement::AnyDisplay,
        display_requirement(),
        allow_interim,
    )
}

pub(crate) fn full_developed_request(artifacts: &ArtifactCache) -> crate::cache::CacheRequest {
    artifacts.request(
        DetailRequirement::NativeDetail,
        RepresentationRequirement::Exact(ArtifactRepresentation::Developed),
        applied_srgb_requirement(),
        false,
    )
}

pub(crate) fn preview_with_priority(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    priority: DecodePriority,
    allow_interim: bool,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let max_size = max_size.max(1);
    let level = if max_size <= 512 {
        RenderLevel::Thumbnail
    } else {
        RenderLevel::Preview
    };
    let artifacts = ArtifactCache::new(path, cache_dir)?;
    let request = preview_request(&artifacts, max_size, allow_interim);
    if let Some(result) = artifacts.lookup(&request, level)? {
        return Ok(result);
    }

    let embedded = produce_embedded(path, &artifacts, max_size, level, cancellation);
    if let Ok(result) = &embedded
        && (allow_interim || result.satisfaction == Some(oxy_domain::MediaSatisfaction::Satisfied))
    {
        return Ok(result.clone());
    }
    artifacts.coordinate_work(
        &request,
        level,
        "raw-compatible-development",
        || cancellation.is_cancelled(),
        |generation| {
            let _decode_permit = acquire_decode(priority, &|| cancellation.is_cancelled())?;
            render_developed(
                path,
                &artifacts,
                Some(max_size),
                level,
                embedded.err().map(|error| error.to_string()),
                generation,
                &request,
                cancellation,
            )
        },
    )
}

pub(crate) fn full(
    path: &Path,
    cache_dir: &Path,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let artifacts = ArtifactCache::new(path, cache_dir)?;
    let hot_request = full_developed_request(&artifacts);
    if let Some(result) = artifacts.lookup(&hot_request, RenderLevel::Full)? {
        return Ok(result);
    }
    let source_size = dimensions(path)?;
    let request = artifacts.request(
        DetailRequirement::Native {
            source: source_size.into(),
        },
        RepresentationRequirement::RawNative {
            allow_camera_preview: true,
        },
        display_requirement(),
        false,
    );
    let production_request = artifacts.request(
        request.detail,
        request.representation,
        request.presentation,
        true,
    );
    if let Some(result) = artifacts.lookup(&request, RenderLevel::Full)? {
        return Ok(result);
    }

    if let Ok(embedded) = produce_embedded(path, &artifacts, 4_096, RenderLevel::Full, cancellation)
        && covers_source(
            ImageDimensions {
                width: embedded.width,
                height: embedded.height,
            },
            source_size,
        )
    {
        return Ok(embedded);
    }
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    artifacts.coordinate_work(
        &hot_request,
        RenderLevel::Full,
        "raw-compatible-development",
        || cancellation.is_cancelled(),
        |generation| {
            let _decode_guard = acquire_raw_full_decode(&|| cancellation.is_cancelled())?;
            render_developed(
                path,
                &artifacts,
                None,
                RenderLevel::Full,
                None,
                generation,
                &production_request,
                cancellation,
            )
        },
    )
}

fn produce_embedded(
    path: &Path,
    artifacts: &ArtifactCache,
    max_size: u32,
    level: RenderLevel,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let embedded_lock = file_lock(&artifacts.source_lock_key("raw-embedded-extract"));
    let _embedded_guard = acquire_file_lock(&embedded_lock, &|| cancellation.is_cancelled())?;
    let request = artifacts.request(
        DetailRequirement::Display {
            min_long_edge: max_size,
        },
        RepresentationRequirement::Exact(ArtifactRepresentation::Embedded),
        display_requirement(),
        true,
    );
    let generation = match artifacts.prepare(&request, level)? {
        ArtifactPreparation::Cached(result) => return Ok(*result),
        ArtifactPreparation::Generate { cache_generation } => cache_generation,
    };
    let revision_id = artifacts.source_revision_id();
    if embedded_extraction_failed(revision_id) {
        return Err(MediaError::CacheArtifact(
            "embedded RAW extraction previously failed for this source revision".into(),
        ));
    }
    let extracted = libraw::embedded(path, max_size).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    });
    let extracted = match extracted {
        Ok(extracted) => extracted,
        Err(error) => {
            record_embedded_extraction_failure(revision_id);
            return Err(error);
        }
    };
    let (bytes, dimensions) = match extracted {
        libraw::Preview::EmbeddedJpeg(data) => {
            let dimensions = {
                let reader = ImageReader::new(Cursor::new(&data)).with_guessed_format()?;
                let mut decoder = reader.into_decoder()?;
                oriented_dimensions(decoder.dimensions(), decoder.orientation()?)
            };
            (Arc::from(data), dimensions)
        }
        libraw::Preview::EmbeddedImage(image) => {
            let destination = artifacts.temporary_output(".jpg")?;
            write_jpeg_atomically(&image, &destination, 90, CAMERA_JPEG)?;
            let dimensions = image::image_dimensions(&destination)?;
            (Arc::from(std::fs::read(&destination)?), dimensions)
        }
    };
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    artifacts.publish(
        bytes,
        dimensions.into(),
        ArtifactRepresentation::Embedded,
        ArtifactPresentation {
            orientation: OrientationState::Metadata,
            color: CacheColorState::EmbeddedOrUnknown,
            sharpening: SharpeningState::None,
        },
        false,
        format!("{RAW_PREVIEW}:embedded:{max_size}"),
        level,
        generation,
        &request,
    )
}

fn embedded_extraction_failed(revision_id: &str) -> bool {
    FAILED_EMBEDDED_REVISIONS
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|candidate| candidate == revision_id)
}

fn record_embedded_extraction_failure(revision_id: &str) {
    let mut failed = FAILED_EMBEDDED_REVISIONS
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    failed.retain(|candidate| candidate != revision_id);
    failed.push_back(revision_id.to_owned());
    while failed.len() > FAILED_EMBEDDED_CAPACITY {
        failed.pop_front();
    }
}

#[allow(clippy::too_many_arguments)]
fn render_developed(
    path: &Path,
    artifacts: &ArtifactCache,
    max_size: Option<u32>,
    level: RenderLevel,
    initial_error: Option<String>,
    generation: u64,
    request: &crate::cache::CacheRequest,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let quality = if level == RenderLevel::Full { 95 } else { 90 };
    let mut errors = initial_error.into_iter().collect::<Vec<_>>();
    for backend in backend_plan(level) {
        let destination = artifacts.temporary_output(".jpg")?;
        let result = match backend {
            #[cfg(target_os = "macos")]
            RawBackend::AppleCoreImage => {
                apple_core_image::render_raw_jpeg(path, &destination, max_size, quality)
            }
            #[cfg(not(target_os = "macos"))]
            RawBackend::AppleCoreImage => Err(MediaError::NativeDecoderUnavailable),
            #[cfg(target_os = "macos")]
            RawBackend::AppleImageIo => {
                apple_image_io::render_jpeg(path, &destination, max_size, quality)
            }
            #[cfg(not(target_os = "macos"))]
            RawBackend::AppleImageIo => Err(MediaError::NativeDecoderUnavailable),
            RawBackend::LibRawDevelopment => libraw::developed(path, max_size)
                .map_err(|message| MediaError::LibRaw {
                    path: path.to_owned(),
                    message,
                })
                .and_then(|image| {
                    let image = if max_size.is_none() {
                        image.unsharpen(0.8, 2)
                    } else {
                        image
                    };
                    write_jpeg_atomically(&image, &destination, quality, RAW_DEVELOPED_JPEG)
                }),
        };
        match result {
            Ok(()) if has_complete_jpeg_markers(&destination)? => {
                if cancellation.is_cancelled() {
                    return Err(MediaError::Cancelled);
                }
                let dimensions = image::image_dimensions(&destination)?.into();
                return artifacts.publish_staged(
                    destination,
                    dimensions,
                    ArtifactRepresentation::Developed,
                    applied_srgb(),
                    max_size.is_none_or(|target| dimensions.width.max(dimensions.height) < target),
                    format!(
                        "{}:{backend:?}:{}",
                        if max_size.is_none() {
                            RAW_FULL
                        } else {
                            RAW_PREVIEW
                        },
                        max_size.unwrap_or_default()
                    ),
                    level,
                    generation,
                    request,
                );
            }
            Ok(()) => errors.push(format!("{backend:?}: produced a truncated JPEG")),
            Err(error) => errors.push(format!("{backend:?}: {error}")),
        }
    }
    Err(MediaError::BackendAttempts {
        attempts: errors.join("; "),
        source: Box::new(MediaError::NativeDecoderUnavailable),
    })
}

fn oriented_dimensions(dimensions: (u32, u32), orientation: Orientation) -> (u32, u32) {
    if matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    ) {
        (dimensions.1, dimensions.0)
    } else {
        dimensions
    }
}

pub(crate) fn covers_source(candidate: ImageDimensions, source: ImageDimensions) -> bool {
    crate::presentation::camera_preview_can_satisfy_raw_full(CAMERA_JPEG, candidate, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_embedded_extraction_is_not_repeated_and_metadata_is_bounded() {
        let prefix = format!("raw-failure-test-{:?}-", std::thread::current().id());
        let first = format!("{prefix}0");
        record_embedded_extraction_failure(&first);
        record_embedded_extraction_failure(&first);
        assert!(embedded_extraction_failed(&first));
        for index in 1..=FAILED_EMBEDDED_CAPACITY {
            record_embedded_extraction_failure(&format!("{prefix}{index}"));
        }
        assert!(!embedded_extraction_failed(&first));
        let failed = FAILED_EMBEDDED_REVISIONS
            .get()
            .unwrap()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(failed.len() <= FAILED_EMBEDDED_CAPACITY);
    }

    #[test]
    fn embedded_jpeg_dimensions_follow_exif_orientation() {
        assert_eq!(
            oriented_dimensions((6_192, 4_128), Orientation::Rotate90),
            (4_128, 6_192)
        );
        assert_eq!(
            oriented_dimensions((6_192, 4_128), Orientation::FlipHorizontal),
            (6_192, 4_128)
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_thumbnail_prefers_measured_image_io_path() {
        assert_eq!(
            backend_plan(RenderLevel::Thumbnail),
            vec![
                RawBackend::AppleImageIo,
                RawBackend::AppleCoreImage,
                RawBackend::LibRawDevelopment,
            ]
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_detail_prefers_measured_core_image_path() {
        for level in [RenderLevel::Preview, RenderLevel::Full] {
            assert_eq!(
                backend_plan(level),
                vec![
                    RawBackend::AppleCoreImage,
                    RawBackend::AppleImageIo,
                    RawBackend::LibRawDevelopment,
                ]
            );
        }
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn portable_platforms_use_libraw_development() {
        for level in [
            RenderLevel::Thumbnail,
            RenderLevel::Preview,
            RenderLevel::Full,
        ] {
            assert_eq!(backend_plan(level), vec![RawBackend::LibRawDevelopment]);
        }
    }
}
