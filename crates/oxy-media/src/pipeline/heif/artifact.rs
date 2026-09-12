use super::planner::{
    full_decoded_request, full_display_request, preview_request, sony_embedded_request,
};
use super::{
    BackendExecutionError, HeifBackend as PlannedHeifBackend, HeifOperation, execute_backend_plan,
    format_attempt_diagnostics, probe_backend_plan,
};
#[cfg(target_os = "macos")]
use crate::backends::apple_image_io;
#[cfg(target_os = "windows")]
use crate::backends::windows_wic;
#[cfg(target_os = "macos")]
use crate::pipeline::artifact::applied_srgb;
use crate::{
    MediaError,
    backends::{ffmpeg_heif, libheif},
    cache::write_jpeg_atomically,
    cache::{
        ArtifactPresentation, ArtifactRequirement, CacheColorState, DetailRequirement, ImageOrigin,
        OrientationState, SharpeningState,
    },
    decode_control::{DecodePriority, acquire_decode, acquire_file_lock, file_lock},
    formats::heif::quirks::sony,
    pipeline::artifact::{ArtifactCache, ArtifactPreparation, duration_ms},
    policy::{HEIF_FULL, HEIF_PREVIEW},
    presentation::{HEIF_DECODED_JPEG, HEIF_UNCONVERTED_JPEG},
};
use image::DynamicImage;
use oxy_domain::{PreviewDiagnostics, PreviewResult, RenderLevel};
use oxy_runtime::CancellationToken;
use std::{
    borrow::Cow,
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

const fn unconverted_presentation() -> ArtifactPresentation {
    ArtifactPresentation {
        geometry: None,
        orientation: OrientationState::Applied,
        color: CacheColorState::EmbeddedOrUnknown,
        sharpening: SharpeningState::None,
    }
}

pub(crate) const fn backend_presentation(backend: PlannedHeifBackend) -> ArtifactPresentation {
    match backend {
        #[cfg(target_os = "macos")]
        PlannedHeifBackend::Platform(_) => applied_srgb(),
        PlannedHeifBackend::CachedArtifact
        | PlannedHeifBackend::Ffmpeg
        | PlannedHeifBackend::FfmpegRgbaFallback
        | PlannedHeifBackend::Libheif => unconverted_presentation(),
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        PlannedHeifBackend::Platform(_) => unconverted_presentation(),
    }
}

const fn jpeg_contract(
    presentation: ArtifactPresentation,
) -> crate::presentation::ArtifactContract {
    match presentation.color {
        CacheColorState::Srgb => HEIF_DECODED_JPEG,
        CacheColorState::EmbeddedOrUnknown => HEIF_UNCONVERTED_JPEG,
    }
}

/// Chooses the presentation boundary for HEIF full detail. The frontend does
/// not maintain a second platform policy table.
pub fn full_uses_artifact(
    path: &Path,
    cache_dir: &Path,
    display_sharpening: bool,
) -> Result<bool, MediaError> {
    let display_sharpening = sony::display_sharpening(path, display_sharpening);
    if cached_heif_full_display(path, cache_dir, display_sharpening)?.is_some() {
        return Ok(true);
    }
    #[cfg(target_os = "macos")]
    return Ok(!display_sharpening);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    priority: DecodePriority,
    try_fast_jpeg: bool,
    allow_decode: bool,
    allow_interim: bool,
    require_native_detail: bool,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let total_started = Instant::now();
    let max_size = max_size.max(1);
    let level = if max_size <= 512 {
        RenderLevel::Thumbnail
    } else {
        RenderLevel::Preview
    };
    let artifacts = ArtifactCache::new(path, cache_dir)?;
    let detail = if require_native_detail {
        DetailRequirement::NativeDetail
    } else {
        DetailRequirement::Display {
            // ImageIO may round the requested edge down by one pixel.
            // This is display tolerance, never proof of native detail.
            min_long_edge: max_size.saturating_sub(1).max(1),
        }
    };
    let satisfied_request = preview_request(&artifacts, detail, false);
    let production_request = preview_request(&artifacts, detail, allow_interim);
    // Embedded Sony results have their own lookup/version above. Reuse only
    // decoded interim results here, never the obsolete padded JPEG policy.
    let mut decoded_interim_request = production_request.clone();
    decoded_interim_request.artifact = ArtifactRequirement::Exact(ImageOrigin::PrimaryImage);
    let mut alternatives = Vec::new();
    if try_fast_jpeg && allow_interim {
        alternatives.push(sony_embedded_request(&artifacts, detail));
    }
    if require_native_detail && allow_interim {
        alternatives.push(decoded_interim_request.clone());
    }
    if let ArtifactPreparation::Cached(result) =
        artifacts.prepare_candidates(&satisfied_request, &alternatives, level)?
    {
        return Ok(*result);
    }

    if try_fast_jpeg && allow_interim {
        let fast_lock = file_lock(&artifacts.source_lock_key("heif-fast-embedded"));
        let _fast_guard = acquire_file_lock(&fast_lock, &|| cancellation.is_cancelled())?;
        let generation =
            match artifacts.prepare_candidates(&satisfied_request, &alternatives, level)? {
                ArtifactPreparation::Cached(result) => return Ok(*result),
                ArtifactPreparation::Generate { cache_generation } => cache_generation,
            };
        if let Ok(inspection) = sony::inspect(path, None)
            && let Some(image) = inspection.embedded_jpeg
        {
            let facts = image.facts;
            let decode_ms = duration_ms(total_started);
            let mut result = artifacts.publish(
                Arc::from(image.bytes),
                facts,
                ArtifactPresentation {
                    geometry: image.geometry,
                    orientation: OrientationState::Metadata,
                    color: CacheColorState::EmbeddedOrUnknown,
                    sharpening: SharpeningState::None,
                },
                format!("{HEIF_PREVIEW}:sony-embedded"),
                level,
                generation,
                &sony_embedded_request(&artifacts, detail),
            )?;
            result.diagnostics = Some(PreviewDiagnostics {
                backend: Some("Sony HIF embedded JPEG".into()),
                queue_wait_ms: Some(0),
                source_wait_ms: Some(0),
                decode_ms: Some(decode_ms),
                encode_ms: Some(0),
                cache_sync_ms: None,
                cache_commit_ms: None,
                total_ms: Some(duration_ms(total_started)),
                fallback_reason: None,
                ..Default::default()
            });
            return Ok(result);
        }
    }
    if !allow_decode {
        return Err(MediaError::NativeDecoderUnavailable);
    }

    let queue_started = Instant::now();
    let source_wait_started = Instant::now();
    let work_request = if require_native_detail && allow_interim {
        &decoded_interim_request
    } else {
        &satisfied_request
    };
    artifacts.coordinate_work(
        work_request,
        level,
        "heif-source-decode",
        || cancellation.is_cancelled(),
        |generation| {
            let source_wait_ms = duration_ms(source_wait_started);
            let decode_permit = acquire_decode(
                if require_native_detail {
                    RenderLevel::Full
                } else {
                    level
                },
                priority,
                &|| cancellation.is_cancelled(),
            )?;
            let queue_wait_ms = duration_ms(queue_started);
            let decode_started = Instant::now();
            let decoded = decode_preview(path, max_size, allow_interim, cancellation)?;
            let decode_ms = duration_ms(decode_started);
            drop(decode_permit);
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            let encode_started = Instant::now();
            let destination = artifacts.temporary_output(".jpg")?;
            write_jpeg_atomically(
                &decoded.image,
                &destination,
                90,
                jpeg_contract(decoded.presentation),
            )?;
            let encode_ms = duration_ms(encode_started);
            let dimensions = image::image_dimensions(&destination)?.into();
            let mut facts = decoded.facts;
            facts.encoded_dimensions = oxy_domain::EncodedDimensions(dimensions);
            facts.exif_orientation = 1;
            facts.byte_integrity = oxy_domain::ByteIntegrity::Reencoded;
            facts.processing.push(oxy_domain::ImageOperation::Encode {
                format: "jpeg".into(),
            });
            if decoded.presentation.color == CacheColorState::Srgb {
                facts
                    .processing
                    .push(oxy_domain::ImageOperation::ColorConvert {
                        target: "sRGB".into(),
                    });
            }

            let mut result = artifacts.publish_staged(
                destination,
                facts.clone(),
                decoded.presentation,
                format!("{HEIF_PREVIEW}:decoded:{max_size}"),
                level,
                generation,
                &production_request,
            )?;
            result.diagnostics = Some(PreviewDiagnostics {
                backend: Some(decoded.backend.into()),
                queue_wait_ms: Some(queue_wait_ms),
                source_wait_ms: Some(source_wait_ms),
                decode_ms: Some(decode_ms),
                encode_ms: Some(encode_ms),
                cache_sync_ms: None,
                cache_commit_ms: None,
                total_ms: Some(duration_ms(total_started)),
                fallback_reason: decoded.fallback_reason,
                ..Default::default()
            });
            Ok(result)
        },
    )
}

struct PreviewDecode {
    image: DynamicImage,
    backend: &'static str,
    fallback_reason: Option<String>,
    facts: oxy_domain::ArtifactFacts,
    presentation: ArtifactPresentation,
}

#[cfg(target_os = "macos")]
fn platform_preview(
    path: &Path,
    image: DynamicImage,
) -> Result<crate::media_source::DecodedImage, MediaError> {
    let source = oxy_domain::DisplayDimensions(libheif::dimensions(path)?);
    let mut facts = crate::media_source::decoded_facts(
        ImageOrigin::PrimaryImage,
        "primary".into(),
        source,
        source,
        oxy_domain::DisplayDimensions((image.width(), image.height()).into()),
        "Apple ImageIO",
    );
    // Thumbnail API does not prove the primary has retained all native samples.
    facts.detail.sampling = oxy_domain::Sampling::Reduced;
    Ok(crate::media_source::DecodedImage { image, facts })
}

fn decode_preview(
    path: &Path,
    max_size: u32,
    allow_interim: bool,
    cancellation: &CancellationToken,
) -> Result<PreviewDecode, MediaError> {
    let plan = probe_backend_plan(path, HeifOperation::Preview);
    let result = execute_backend_plan(
        &plan,
        || cancellation.is_cancelled(),
        |backend| match backend {
            PlannedHeifBackend::CachedArtifact => Err(MediaError::NativeDecoderUnavailable),
            #[cfg(target_os = "macos")]
            PlannedHeifBackend::Platform(_) => apple_image_io::decode_rgba8(path, max_size)
                .and_then(|image| platform_preview(path, image)),
            #[cfg(target_os = "windows")]
            PlannedHeifBackend::Platform(_) => windows_wic::decode_full_rgba8(path)
                .map(|image| image.thumbnail(max_size, max_size))
                .and_then(|image| {
                    let source = oxy_domain::DisplayDimensions(libheif::dimensions(path)?);
                    let facts = crate::media_source::decoded_facts(
                        ImageOrigin::PrimaryImage,
                        "primary".into(),
                        source,
                        source,
                        oxy_domain::DisplayDimensions((image.width(), image.height()).into()),
                        "Windows WIC",
                    );
                    Ok(crate::media_source::DecodedImage { image, facts })
                }),
            #[cfg(target_os = "linux")]
            PlannedHeifBackend::Platform(_) => Err(MediaError::NativeDecoderUnavailable),
            PlannedHeifBackend::Ffmpeg | PlannedHeifBackend::FfmpegRgbaFallback => {
                ffmpeg_heif::decode_scaled_preview(path, max_size)
            }
            PlannedHeifBackend::Libheif => libheif::decode_scaled(path, max_size, allow_interim),
        },
    )
    .map_err(BackendExecutionError::into_media_error)?;
    let backend = match result.backend {
        PlannedHeifBackend::CachedArtifact => "cached full JPEG",
        PlannedHeifBackend::Platform(oxy_domain::HeifBackendKind::AppleImageIo) => {
            "Apple ImageIO thumbnail"
        }
        PlannedHeifBackend::Platform(oxy_domain::HeifBackendKind::WindowsWic) => {
            "Windows WIC thumbnail"
        }
        PlannedHeifBackend::Platform(_) => "platform HEIF thumbnail",
        PlannedHeifBackend::Ffmpeg | PlannedHeifBackend::FfmpegRgbaFallback => {
            "FFmpeg auxiliary preview"
        }
        PlannedHeifBackend::Libheif => "libheif scaled preview",
    };
    Ok(PreviewDecode {
        image: result.value.image,
        backend,
        fallback_reason: format_attempt_diagnostics(&result.diagnostics),
        facts: result.value.facts,
        presentation: backend_presentation(result.backend),
    })
}

fn full_output_facts(
    path: &Path,
    output: oxy_domain::PixelDimensions,
    backend: &str,
    presentation: ArtifactPresentation,
) -> Result<oxy_domain::ArtifactFacts, MediaError> {
    let source = oxy_domain::DisplayDimensions(libheif::dimensions(path)?);
    let mut facts = crate::media_source::decoded_facts(
        ImageOrigin::PrimaryImage,
        "primary".into(),
        source,
        source,
        oxy_domain::DisplayDimensions(output),
        backend,
    );
    if presentation.color == CacheColorState::Srgb {
        facts
            .processing
            .push(oxy_domain::ImageOperation::ColorConvert {
                target: "sRGB".into(),
            });
    }
    if presentation.sharpening == SharpeningState::Display {
        facts.processing.push(oxy_domain::ImageOperation::Sharpen);
    }
    facts.processing.push(oxy_domain::ImageOperation::Encode {
        format: "jpeg".into(),
    });
    facts.byte_integrity = oxy_domain::ByteIntegrity::Reencoded;
    Ok(facts)
}

pub(crate) fn full(
    path: &Path,
    cache_dir: &Path,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let artifacts = ArtifactCache::new(path, cache_dir)?;
    let request = full_decoded_request(&artifacts);
    if let Some(result) = artifacts.lookup(&request, RenderLevel::Full)? {
        return Ok(result);
    }
    artifacts.coordinate_work(
        &request,
        RenderLevel::Full,
        "heif-source-decode",
        || cancellation.is_cancelled(),
        |generation| {
            let _decode_permit =
                acquire_decode(RenderLevel::Full, DecodePriority::Foreground, &|| {
                    cancellation.is_cancelled()
                })?;
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            let destination = artifacts.temporary_output(".jpg")?;
            let (backend, fallback_reason, presentation) =
                transcode_heif_source(path, &destination, 95, || cancellation.is_cancelled())?;
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            let dimensions = image::image_dimensions(&destination)?.into();
            let facts = full_output_facts(path, dimensions, backend, presentation)?;

            let mut result = artifacts.publish_staged(
                destination,
                facts,
                presentation,
                format!("{HEIF_FULL}:native"),
                RenderLevel::Full,
                generation,
                &request,
            )?;
            result.diagnostics = Some(PreviewDiagnostics {
                backend: Some(backend.into()),
                fallback_reason,
                ..PreviewDiagnostics::default()
            });
            Ok(result)
        },
    )
}

#[cfg(test)]
pub(crate) fn cached_heif_full(
    path: &Path,
    cache_dir: &Path,
) -> Result<Option<PreviewResult>, MediaError> {
    cached_heif_full_display(path, cache_dir, false)
}

pub(crate) fn cached_heif_full_display(
    path: &Path,
    cache_dir: &Path,
    display_sharpening: bool,
) -> Result<Option<PreviewResult>, MediaError> {
    let artifacts = ArtifactCache::new(path, cache_dir)?;
    let display_sharpening = sony::display_sharpening(path, display_sharpening);
    let request = full_display_request(&artifacts, display_sharpening);
    artifacts.lookup(&request, RenderLevel::Full)
}

/// A full-display cache hit is already presentation-ready. Register its leased
/// resource without routing Display variants through the unsharpened preview queue.
pub fn cached_heif_full_for_display(
    path: &Path,
    cache_dir: &Path,
    display_sharpening: bool,
) -> Result<Option<PreviewResult>, MediaError> {
    crate::pipeline::artifact::with_app_publication(|| {
        cached_heif_full_display(path, cache_dir, display_sharpening)
    })
    .0
}

#[cfg(test)]
pub(crate) fn cache_full(path: &Path, cache_dir: &Path) -> Result<PathBuf, MediaError> {
    if let Some(cached) = cached_heif_full(path, cache_dir)? {
        return Ok(cached.path);
    }
    full(path, cache_dir, &CancellationToken::default()).map(|result| result.path)
}

/// Persists canonical, orientation-applied pixels already obtained by a tile
/// session. The supplied presentation records whether the backend verified an
/// sRGB transform; fallback pixels remain unprofiled rather than relabeled.
/// This path must not reopen or decode the HEIF source.
pub(crate) fn cache_full_image(
    source_revision: &crate::cache::SourceRevision,
    cache_dir: &Path,
    image: &DynamicImage,
    presentation: ArtifactPresentation,
    backend: &str,
) -> Result<PathBuf, MediaError> {
    cache_full_image_from(
        source_revision,
        cache_dir,
        presentation,
        backend,
        || false,
        || Ok(Cow::Borrowed(image)),
    )
}

/// Admit before assembling pixels, so clearing the cache during assembly cannot
/// resurrect an artifact from the old generation. The producer runs only on a miss.
pub(crate) fn cache_full_image_from<'a>(
    source_revision: &crate::cache::SourceRevision,
    cache_dir: &Path,
    presentation: ArtifactPresentation,
    backend: &str,
    cancelled: impl Fn() -> bool,
    produce: impl FnOnce() -> Result<Cow<'a, DynamicImage>, MediaError>,
) -> Result<PathBuf, MediaError> {
    cache_full_output(
        source_revision,
        cache_dir,
        presentation,
        backend,
        &cancelled,
        |destination| {
            let image = produce()?;
            if cancelled() {
                return Err(MediaError::Cancelled);
            }
            write_session_jpeg(&image, destination, presentation)?;
            Ok(vec![oxy_domain::ImageOperation::Encode {
                format: "jpeg".into(),
            }])
        },
    )
}

pub(crate) fn write_session_jpeg(
    image: &DynamicImage,
    destination: &Path,
    presentation: ArtifactPresentation,
) -> Result<(), MediaError> {
    if presentation.color == CacheColorState::Srgb {
        #[cfg(target_os = "macos")]
        apple_image_io::write_jpeg(image, destination, 95)?;
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        write_jpeg_atomically(image, destination, 95, HEIF_DECODED_JPEG)?;
    } else {
        write_jpeg_atomically(image, destination, 95, HEIF_UNCONVERTED_JPEG)?;
    }
    Ok(())
}

/// Prepares the generation fence before invoking an encoder, then validates and
/// atomically publishes its temporary output using the same artifact contract.
pub(crate) fn cache_full_output(
    source_revision: &crate::cache::SourceRevision,
    cache_dir: &Path,
    presentation: ArtifactPresentation,
    backend: &str,
    cancelled: impl Fn() -> bool,
    write_output: impl FnOnce(&Path) -> Result<Vec<oxy_domain::ImageOperation>, MediaError>,
) -> Result<PathBuf, MediaError> {
    let path = &source_revision.canonical_path;
    if cancelled() {
        return Err(MediaError::Cancelled);
    }
    if crate::cache::SourceRevision::observe(path)? != *source_revision {
        return Err(MediaError::StaleSourceRevision);
    }
    let artifacts = ArtifactCache::for_source_revision(source_revision.clone(), cache_dir)?;
    let request = super::planner::full_output_request(&artifacts, presentation);
    let generation = match artifacts.prepare(&request, RenderLevel::Full)? {
        ArtifactPreparation::Cached(result) => return Ok(result.path),
        ArtifactPreparation::Generate { cache_generation } => cache_generation,
    };
    let destination = artifacts.temporary_output(".jpg")?;
    let encode_started = Instant::now();
    let output_operations = write_output(&destination)?;
    let encode_ms = encode_started.elapsed().as_secs_f64() * 1000.0;
    if cancelled() {
        return Err(MediaError::Cancelled);
    }
    let source_dimensions = libheif::dimensions(path)?;
    let dimensions = image::image_dimensions(&destination)?;
    if dimensions != (source_dimensions.width, source_dimensions.height) {
        return Err(MediaError::CacheArtifact(
            "HEIF session pixels do not contain native display detail".into(),
        ));
    }
    if cancelled() {
        return Err(MediaError::Cancelled);
    }
    let mut facts = full_output_facts(path, dimensions.into(), backend, presentation)?;
    // The session reports its actual assembly/encoding route after success.
    facts.processing.pop();
    facts.processing.extend(output_operations);
    let commit_started = Instant::now();
    let result = artifacts.publish_staged(
        destination,
        facts,
        presentation,
        format!("{HEIF_FULL}:native"),
        RenderLevel::Full,
        generation,
        &request,
    )?;
    eprintln!(
        "HEIF full cache: encode_ms={encode_ms:.3} commit_ms={:.3}",
        commit_started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(result.path)
}

fn transcode_heif_source(
    source: &Path,
    destination: &Path,
    quality: u8,
    cancelled: impl Fn() -> bool,
) -> Result<(&'static str, Option<String>, ArtifactPresentation), MediaError> {
    let plan = probe_backend_plan(source, HeifOperation::FullArtifact);
    let result = execute_backend_plan(&plan, &cancelled, |backend| {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(destination)?;
        match backend {
            PlannedHeifBackend::CachedArtifact => Err(MediaError::NativeDecoderUnavailable),
            #[cfg(target_os = "macos")]
            PlannedHeifBackend::Platform(_) => {
                apple_image_io::transcode_jpeg(source, destination, quality)
            }
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            PlannedHeifBackend::Platform(_) => Err(MediaError::NativeDecoderUnavailable),
            PlannedHeifBackend::Ffmpeg | PlannedHeifBackend::FfmpegRgbaFallback => {
                ffmpeg_heif::transcode_full_jpeg(
                    source,
                    destination,
                    libheif::dimensions(source)?,
                    quality,
                    &cancelled,
                )
            }
            PlannedHeifBackend::Libheif => Err(MediaError::NativeDecoderUnavailable),
        }
    })
    .map_err(BackendExecutionError::into_media_error)?;
    let backend = match result.backend {
        PlannedHeifBackend::CachedArtifact => "cached full JPEG",
        PlannedHeifBackend::Platform(oxy_domain::HeifBackendKind::AppleImageIo) => "Apple ImageIO",
        PlannedHeifBackend::Platform(_) => "platform HEIF backend",
        PlannedHeifBackend::Ffmpeg | PlannedHeifBackend::FfmpegRgbaFallback => "FFmpeg",
        PlannedHeifBackend::Libheif => "libheif",
    };
    Ok((
        backend,
        format_attempt_diagnostics(&result.diagnostics),
        backend_presentation(result.backend),
    ))
}

#[cfg(test)]
mod delivery_tests {
    use super::*;

    #[test]
    fn embedded_or_resampled_pixels_do_not_establish_primary_native_detail() {
        let source = oxy_domain::DisplayDimensions((6000, 4000).into());
        for origin in [ImageOrigin::PrimaryImage, ImageOrigin::EmbeddedPreview] {
            let facts = crate::media_source::decoded_facts(
                origin,
                "item".into(),
                source,
                source,
                oxy_domain::DisplayDimensions((511, 341).into()),
                "test-decoder",
            );
            assert!(!facts.native_detail());
            let mut upscaled = facts;
            upscaled.resize(source);
            assert!(!upscaled.native_detail());
        }
    }

    #[test]
    fn fallback_backends_never_claim_an_unverified_srgb_transform() {
        for backend in [
            PlannedHeifBackend::Ffmpeg,
            PlannedHeifBackend::FfmpegRgbaFallback,
            PlannedHeifBackend::Libheif,
        ] {
            let presentation = backend_presentation(backend);
            assert_eq!(presentation.color, CacheColorState::EmbeddedOrUnknown);
            assert_eq!(jpeg_contract(presentation), HEIF_UNCONVERTED_JPEG);
        }

        #[cfg(target_os = "macos")]
        assert_eq!(
            backend_presentation(PlannedHeifBackend::Platform(
                oxy_domain::HeifBackendKind::AppleImageIo,
            ))
            .color,
            CacheColorState::Srgb
        );
    }

    #[test]
    fn unconverted_wide_gamut_fallback_is_unprofiled_and_cannot_satisfy_srgb() {
        use image::ImageDecoder;

        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("fallback.jpg");
        let presentation = backend_presentation(PlannedHeifBackend::Libheif);
        write_jpeg_atomically(
            &DynamicImage::new_rgb8(8, 4),
            &destination,
            90,
            jpeg_contract(presentation),
        )
        .unwrap();
        let mut decoder = image::ImageReader::open(&destination)
            .unwrap()
            .into_decoder()
            .unwrap();
        assert!(decoder.icc_profile().unwrap().is_none());

        let source_path = directory.path().join("wide-gamut.heif");
        std::fs::write(&source_path, b"wide-gamut source identity").unwrap();
        let source_revision = crate::cache::SourceRevision::observe(&source_path).unwrap();
        let artifact = crate::cache::MediaArtifact {
            artifact_id: "fallback".into(),
            source_revision: source_revision.clone(),
            variant: crate::cache::VariantIdentity {
                presentation,
                policy_revision: crate::cache::MEDIA_CACHE_POLICY_REVISION,
                target: "fallback".into(),
            },
            facts: crate::media_source::test_facts(ImageOrigin::PrimaryImage, (8, 4).into(), true),
            byte_size: std::fs::metadata(&destination).unwrap().len(),
            media_type: "image/jpeg".into(),
            location: crate::cache::ArtifactLocation::Managed(destination),
        };
        let srgb_request = crate::cache::CacheRequest {
            source_revision,
            detail: DetailRequirement::NativeDetail,
            artifact: ArtifactRequirement::Exact(ImageOrigin::PrimaryImage),
            presentation: crate::pipeline::artifact::applied_srgb_requirement(),
            policy_revision: crate::cache::MEDIA_CACHE_POLICY_REVISION,
            allow_interim: false,
        };
        assert!(crate::cache::satisfies(&artifact, &srgb_request).is_none());
    }

    #[test]
    fn display_full_lookup_is_exact_and_does_not_rebuild_a_cached_variant() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("display.heif");
        std::fs::write(&source, b"\0\0\0\x14ftypheic\0\0\0\0SHIF").unwrap();
        let artifacts = ArtifactCache::new(&source, directory.path()).unwrap();
        let request = full_display_request(&artifacts, true);
        let mut presentation = unconverted_presentation();
        presentation.sharpening = SharpeningState::Display;
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&DynamicImage::new_rgb8(8, 4))
            .unwrap();
        let generation = match artifacts.prepare(&request, RenderLevel::Full).unwrap() {
            ArtifactPreparation::Generate { cache_generation } => cache_generation,
            ArtifactPreparation::Cached(_) => panic!("fresh cache"),
        };
        let stored = artifacts
            .publish(
                Arc::from(jpeg),
                crate::media_source::test_facts(ImageOrigin::PrimaryImage, (8, 4).into(), true),
                presentation,
                format!("{HEIF_FULL}:native"),
                RenderLevel::Full,
                generation,
                &request,
            )
            .unwrap();
        assert!(
            cached_heif_full_display(&source, directory.path(), false)
                .unwrap()
                .is_none()
        );
        assert!(
            artifacts
                .lookup(
                    &preview_request(
                        &artifacts,
                        DetailRequirement::Display { min_long_edge: 4 },
                        false
                    ),
                    RenderLevel::Preview
                )
                .unwrap()
                .is_none()
        );
        let cached = cached_heif_full_for_display(&source, directory.path(), true)
            .unwrap()
            .unwrap();
        assert_eq!(cached.path, stored.path);
        assert!(cached.resource.is_some());
        assert!(full_uses_artifact(&source, directory.path(), true).unwrap());
        let path = cache_full_image_from(
            &crate::cache::SourceRevision::observe(&source).unwrap(),
            directory.path(),
            presentation,
            "test decoder",
            || false,
            || panic!("a cache hit must not assemble pixels"),
        )
        .unwrap();
        assert_eq!(path, cached.path);
        crate::shared_resource_registry().release(&cached.resource.unwrap().resource_id);
    }

    #[test]
    fn failed_output_encoder_removes_partial_artifact() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("cancelled.heif");
        std::fs::write(&source, b"source revision fixture").unwrap();
        let revision = crate::cache::SourceRevision::observe(&source).unwrap();
        let partial = std::cell::RefCell::new(PathBuf::new());
        let result = cache_full_output(
            &revision,
            directory.path(),
            unconverted_presentation(),
            "test decoder",
            || false,
            |destination| {
                *partial.borrow_mut() = destination.to_owned();
                std::fs::write(destination, b"partial jpeg")?;
                Err(MediaError::Cancelled)
            },
        );
        assert!(matches!(result, Err(MediaError::Cancelled)));
        assert!(!partial.borrow().exists());
        assert!(
            cached_heif_full(&source, directory.path())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sharpening_uses_tiles_when_no_display_variant_exists() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.heif");
        std::fs::write(&source, b"\0\0\0\x14ftypheic\0\0\0\0SHIF").unwrap();

        assert!(!full_uses_artifact(&source, directory.path(), true).unwrap());
    }
}
