pub(crate) mod planner;

use planner::preview_request;
use planner::{
    LARGEST_EMBEDDED_JPEG_TARGET, RawBackend, RawFullPlan, display_requirement, plan_backends,
};

#[cfg(target_os = "macos")]
use crate::backends::{apple_core_image, apple_image_io};
use crate::{
    ImageDimensions, MediaError,
    backends::libraw,
    cache::write_jpeg_atomically,
    cache::{
        ArtifactPresentation, ArtifactRequirement, CacheColorState, DetailRequirement, ImageOrigin,
        OrientationState, SharpeningState,
    },
    decode_control::{
        DecodePriority, acquire_decode, acquire_file_lock, acquire_raw_full_decode, file_lock,
    },
    media_source::has_complete_jpeg_markers,
    pipeline::artifact::{ArtifactCache, ArtifactPreparation, applied_srgb},
    policy::{RAW_PREVIEW, RAW_THUMBNAIL},
    presentation::{CAMERA_JPEG, RAW_DEVELOPED_JPEG},
};
#[cfg(test)]
use image::metadata::Orientation;
use image::{ImageDecoder, ImageReader};
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

pub(crate) fn dimensions(path: &Path) -> Result<ImageDimensions, MediaError> {
    libraw::dimensions(path).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    })
}

pub(crate) fn preview(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    priority: DecodePriority,
    allow_interim: bool,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    if let Some(max_size) = planner::preview_max_size(level) {
        preview_with_priority(
            path,
            cache_dir,
            max_size,
            priority,
            allow_interim,
            cancellation,
        )
    } else {
        full_with_interim(path, cache_dir, allow_interim, cancellation)
    }
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
    if level == RenderLevel::Preview {
        // Preview means the best camera JPEG, even when it is below 4096 px.
        // Its exact target proves that selection is complete; a thumbnail or
        // a merely adequate JPEG must not satisfy this request.
        let request = planner::largest_preview_request(&artifacts);
        match produce_embedded_with_request(
            path,
            &artifacts,
            0,
            level,
            &request,
            priority,
            cancellation,
        ) {
            Ok(result) => return Ok(result),
            Err(MediaError::LibRaw { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    let mut request = preview_request(&artifacts, max_size, allow_interim);
    if level == RenderLevel::Thumbnail {
        request.artifact = ArtifactRequirement::BoundedThumbnail {
            target: RAW_THUMBNAIL,
        };
        request.allow_interim = false;
    }
    if let Some(result) = artifacts.lookup(&request, level)? {
        return Ok(result);
    }

    let embedded = produce_embedded(path, &artifacts, max_size, level, priority, cancellation);
    if let Ok(result) = &embedded
        && (allow_interim || result.satisfaction == Some(oxy_domain::MediaSatisfaction::Satisfied))
    {
        return Ok(result.clone());
    }
    if let Err(error) = &embedded
        && matches!(
            error,
            MediaError::Cancelled
                | MediaError::ResourceBudgetExhausted { .. }
                | MediaError::StaleSourceRevision
                | MediaError::StaleCacheGeneration
                | MediaError::Io(_)
        )
    {
        return embedded;
    }
    artifacts.coordinate_work(
        &request,
        level,
        "raw-compatible-development",
        || cancellation.is_cancelled(),
        |generation| {
            let _decode_permit = acquire_decode(level, priority, &|| cancellation.is_cancelled())?;
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

#[cfg(test)]
pub(crate) fn full(
    path: &Path,
    cache_dir: &Path,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    full_with_interim(path, cache_dir, false, cancellation)
}

pub(crate) fn full_with_interim(
    path: &Path,
    cache_dir: &Path,
    allow_interim: bool,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let artifacts = ArtifactCache::new(path, cache_dir)?;
    let source = dimensions(path);
    #[cfg(target_os = "windows")]
    let source = source.or_else(|_| crate::backends::windows_wic::raw::dimensions(path));
    let plan = RawFullPlan::new(&artifacts, source?);
    let retry_development = plan.regenerate;
    if retry_development {
        if let Some(result) = artifacts.lookup(&plan.developed, RenderLevel::Full)? {
            return Ok(result);
        }
    } else if let ArtifactPreparation::Cached(result) = artifacts.prepare_candidates(
        &plan.embedded,
        std::slice::from_ref(&plan.developed),
        RenderLevel::Full,
    )? {
        return Ok(*result);
    }
    let production_request = plan.embedded_production();

    if (!retry_development || allow_interim)
        && let Ok(mut embedded) = produce_embedded_with_request(
            path,
            &artifacts,
            0,
            RenderLevel::Full,
            &production_request,
            DecodePriority::Foreground,
            cancellation,
        )
    {
        if !retry_development
            && embedded.satisfaction == Some(oxy_domain::MediaSatisfaction::Satisfied)
        {
            return Ok(embedded);
        }
        if allow_interim {
            // Loupe already retains its thumbnail. Publish the largest camera
            // JPEG before the queue starts WIC/LibRaw's terminal upgrade.
            embedded.satisfaction = Some(oxy_domain::MediaSatisfaction::Interim);
            return Ok(embedded);
        }
    }
    if let Some(result) = artifacts.lookup(&plan.developed, RenderLevel::Full)? {
        return Ok(result);
    }
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    artifacts.coordinate_work(
        &plan.developed,
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
                &plan.developed,
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
    priority: DecodePriority,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let request = artifacts.request(
        DetailRequirement::Display {
            min_long_edge: max_size,
        },
        if level == RenderLevel::Thumbnail {
            ArtifactRequirement::BoundedThumbnail {
                target: RAW_THUMBNAIL,
            }
        } else {
            ArtifactRequirement::Exact(ImageOrigin::EmbeddedPreview)
        },
        display_requirement(),
        level != RenderLevel::Thumbnail,
    );
    produce_embedded_with_request(
        path,
        artifacts,
        max_size,
        level,
        &request,
        priority,
        cancellation,
    )
}

#[allow(clippy::too_many_arguments)]
fn produce_embedded_with_request(
    path: &Path,
    artifacts: &ArtifactCache,
    max_size: u32,
    level: RenderLevel,
    request: &crate::cache::CacheRequest,
    priority: DecodePriority,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let embedded_lock = file_lock(&artifacts.source_lock_key("raw-embedded-extract"));
    let _embedded_guard = acquire_file_lock(&embedded_lock, &|| cancellation.is_cancelled())?;
    // Ordinary interim previews must not suppress extraction of better pixels.
    // The planner target proves embedded selection is exhausted, even when the
    // largest JPEG is too small for full; reuse it before development fallback.
    let mut completed_request = request.clone();
    completed_request.allow_interim = max_size == 0;
    let generation = match artifacts.prepare(&completed_request, level)? {
        ArtifactPreparation::Cached(result) => return Ok(*result),
        ArtifactPreparation::Generate { cache_generation } => cache_generation,
    };
    let revision_id = artifacts.source_revision_id();
    if level == RenderLevel::Thumbnail {
        let derivation = preview_request(artifacts, max_size, false);
        if let Some(cached) = artifacts.lookup(&derivation, level)? {
            let super::jpeg_transform::EncodedJpegThumbnail {
                facts,
                bytes,
                dimensions: _,
                mut presentation,
            } = super::thumbnail::encode_cached_jpeg_thumbnail(&cached, priority, cancellation)?;
            // Preserve the existing RAW cache presentation identity.
            presentation.color = CacheColorState::EmbeddedOrUnknown;
            return artifacts.publish(
                bytes,
                facts,
                presentation,
                RAW_THUMBNAIL.into(),
                level,
                generation,
                request,
            );
        }
    }
    if max_size != 0 && embedded_extraction_failed(revision_id) {
        return Err(MediaError::CacheArtifact(
            "embedded RAW extraction previously failed for this source revision".into(),
        ));
    }
    let extracted = libraw::embedded(path, max_size).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    });
    let (extracted, reference) = match extracted {
        Ok(extracted) => extracted,
        Err(error) => {
            if max_size != 0 {
                record_embedded_extraction_failure(revision_id);
            }
            return Err(error);
        }
    };
    let (bytes, mut facts) = match extracted {
        libraw::Preview::EmbeddedJpeg(data) => {
            use sha2::{Digest, Sha256};
            let candidate_id = format!("libraw-jpeg:{:x}", Sha256::digest(&data));
            if level == RenderLevel::Thumbnail {
                let super::jpeg_transform::EncodedJpegThumbnail {
                    bytes, mut facts, ..
                } = super::jpeg_transform::encode_jpeg_thumbnail(data, priority, cancellation)?;
                facts.source.candidate_id = candidate_id;
                (bytes, facts)
            } else {
                let mut decoder = ImageReader::new(Cursor::new(&data))
                    .with_guessed_format()?
                    .into_decoder()?;
                let encoded = oxy_domain::EncodedDimensions(decoder.dimensions().into());
                let orientation = decoder.orientation()?.to_exif();
                let facts = crate::media_source::source_facts(
                    ImageOrigin::EmbeddedPreview,
                    candidate_id,
                    encoded,
                    orientation,
                    reference,
                );
                drop(decoder);
                // LibRaw may add orientation metadata. Do not claim original payload bytes.
                (Arc::from(data), facts)
            }
        }
        libraw::Preview::EmbeddedImage(decoded) => {
            let destination = artifacts.temporary_output(".jpg")?;
            write_jpeg_atomically(&decoded.image, &destination, 90, CAMERA_JPEG)?;
            let mut facts = decoded.facts;
            facts.processing.push(oxy_domain::ImageOperation::Encode {
                format: "jpeg".into(),
            });
            facts.byte_integrity = oxy_domain::ByteIntegrity::Reencoded;
            (Arc::from(std::fs::read(&destination)?), facts)
        }
    };
    facts.source.origin = ImageOrigin::EmbeddedPreview;
    facts.detail.reference_dimensions = reference;
    facts.detail.region = oxy_domain::PreviewContentRect {
        x: 0,
        y: 0,
        width: reference.0.width,
        height: reference.0.height,
    };
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    artifacts.publish(
        bytes,
        facts,
        ArtifactPresentation {
            geometry: None,
            orientation: if level == RenderLevel::Thumbnail {
                OrientationState::Applied
            } else {
                OrientationState::Metadata
            },
            color: CacheColorState::EmbeddedOrUnknown,
            sharpening: SharpeningState::None,
        },
        if level == RenderLevel::Thumbnail {
            RAW_THUMBNAIL.into()
        } else if max_size == 0 {
            LARGEST_EMBEDDED_JPEG_TARGET.into()
        } else {
            format!("{RAW_PREVIEW}:embedded:{max_size}")
        },
        level,
        generation,
        request,
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
    for backend in plan_backends(level) {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let destination = artifacts.temporary_output(".jpg")?;
        let mut completed_facts = None;
        let started = std::time::Instant::now();
        let result = match backend {
            #[cfg(target_os = "macos")]
            RawBackend::AppleCoreImage => {
                apple_core_image::render_raw_jpeg(path, &destination, max_size, quality)
            }
            #[cfg(target_os = "macos")]
            RawBackend::AppleImageIo => {
                apple_image_io::render_jpeg(path, &destination, max_size, quality)
            }
            #[cfg(target_os = "windows")]
            RawBackend::WindowsWic => crate::raw_support::decode(path, cancellation)
                .and_then(|decoded| encode_developed(decoded, &destination, quality, cancellation))
                .map(|facts| completed_facts = Some(facts)),
            RawBackend::LibRawDevelopment => libraw::developed(path, max_size, cancellation)
                .map_err(|message| MediaError::LibRaw {
                    path: path.to_owned(),
                    message,
                })
                .and_then(|decoded| encode_developed(decoded, &destination, quality, cancellation))
                .map(|facts| completed_facts = Some(facts)),
        };
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        match result {
            Ok(()) if has_complete_jpeg_markers(&destination)? => {
                if cancellation.is_cancelled() {
                    return Err(MediaError::Cancelled);
                }
                let facts = if let Some(facts) = completed_facts {
                    facts
                } else {
                    let dimensions = image::image_dimensions(&destination)?.into();
                    let reference = oxy_domain::DisplayDimensions(self::dimensions(path)?);
                    let mut facts = crate::media_source::source_facts(
                        ImageOrigin::RawSensor,
                        "raw-sensor".into(),
                        oxy_domain::EncodedDimensions(reference.0),
                        1,
                        reference,
                    );
                    facts.source.encoded_dimensions = None;
                    facts.processing.push(oxy_domain::ImageOperation::Develop {
                        backend: format!("{backend:?}"),
                    });
                    facts
                        .processing
                        .push(oxy_domain::ImageOperation::ColorConvert {
                            target: "sRGB".into(),
                        });
                    facts.resize(oxy_domain::DisplayDimensions(dimensions));
                    facts.processing.push(oxy_domain::ImageOperation::Encode {
                        format: "jpeg".into(),
                    });
                    facts.encoded_dimensions = oxy_domain::EncodedDimensions(dimensions);
                    facts.exif_orientation = 1;
                    facts.byte_integrity = oxy_domain::ByteIntegrity::Reencoded;

                    facts
                };

                let target = if level == RenderLevel::Full {
                    let ArtifactRequirement::ExactVariant { target, .. } = &request.artifact else {
                        return Err(MediaError::CacheArtifact(
                            "full development requires an exact target".into(),
                        ));
                    };
                    target.clone()
                } else if level == RenderLevel::Thumbnail {
                    RAW_THUMBNAIL.into()
                } else {
                    format!("{RAW_PREVIEW}:{backend:?}:{}", max_size.unwrap_or_default())
                };
                let mut result = artifacts.publish_staged(
                    destination,
                    facts,
                    applied_srgb(),
                    target,
                    level,
                    generation,
                    request,
                )?;
                result.diagnostics = Some(oxy_domain::PreviewDiagnostics {
                    backend: Some(format!("{backend:?}")),
                    total_ms: Some(started.elapsed().as_millis() as u64),
                    fallback_reason: (!errors.is_empty()).then(|| errors.join("; ")),
                    ..Default::default()
                });
                return Ok(result);
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

fn encode_developed(
    decoded: crate::media_source::DecodedImage,
    destination: &Path,
    quality: u8,
    cancellation: &CancellationToken,
) -> Result<oxy_domain::ArtifactFacts, MediaError> {
    let mut facts = decoded.facts;
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    // Preserve decoder pixels. Display sharpening was validated only for Sony HIF.
    write_jpeg_atomically(&decoded.image, destination, quality, RAW_DEVELOPED_JPEG)?;
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    facts.processing.push(oxy_domain::ImageOperation::Encode {
        format: "jpeg".into(),
    });
    facts.byte_integrity = oxy_domain::ByteIntegrity::Reencoded;
    Ok(facts)
}

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn covers_source(candidate: ImageDimensions, source: ImageDimensions) -> bool {
    planner::camera_preview_detail(source).accepts(oxy_domain::DisplayDimensions(candidate), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developed_output_encodes_decoder_pixels_without_postprocessing() {
        let directory = tempfile::tempdir().unwrap();
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(32, 32, |x, y| {
            image::Rgb([if x < 16 { 35 } else { 195 }, (y * 7) as u8, 90])
        }));
        let expected = directory.path().join("decoder.jpg");
        write_jpeg_atomically(&image, &expected, 95, RAW_DEVELOPED_JPEG).unwrap();
        let decoded = crate::media_source::DecodedImage {
            facts: crate::media_source::test_facts(ImageOrigin::RawSensor, (32, 32).into(), true),
            image,
        };
        let actual = directory.path().join("output.jpg");
        let facts = encode_developed(decoded, &actual, 95, &CancellationToken::default()).unwrap();
        assert_eq!(
            std::fs::read(actual).unwrap(),
            std::fs::read(expected).unwrap()
        );
        assert_eq!(
            facts.processing,
            vec![oxy_domain::ImageOperation::Encode {
                format: "jpeg".into()
            }]
        );
    }

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
            plan_backends(RenderLevel::Thumbnail),
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
                plan_backends(level),
                vec![
                    RawBackend::AppleCoreImage,
                    RawBackend::AppleImageIo,
                    RawBackend::LibRawDevelopment,
                ]
            );
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn portable_platforms_use_libraw_development() {
        for level in [
            RenderLevel::Thumbnail,
            RenderLevel::Preview,
            RenderLevel::Full,
        ] {
            assert_eq!(plan_backends(level), vec![RawBackend::LibRawDevelopment]);
        }
    }
}
