use super::backend::{
    BackendExecutionError, HeifBackend as PlannedHeifBackend, HeifOperation, backend_plan,
    execute_backend_plan, format_attempt_diagnostics,
};
#[cfg(target_os = "macos")]
use crate::backends::apple_image_io;
#[cfg(target_os = "windows")]
use crate::backends::windows_wic;
use crate::pipeline::artifact::{duration_ms, larger_cached_decoded_preview};
use crate::{
    MediaError,
    backends::{ffmpeg_heif, libheif},
    cache::{
        persist_atomically, preview_cache_key, write_bytes_atomically, write_jpeg_atomically_timed,
    },
    decode_control::{DecodePriority, acquire_decode, acquire_file_lock},
    formats::heif::quirks::sony,
    media_source::preview_result,
    presentation::HEIF_DECODED_JPEG,
};
use image::DynamicImage;
use oxy_domain::{PreviewDiagnostics, PreviewKind, PreviewResult};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::Instant,
};

// Bumped from `libheif-1.23-sdr-v1` (16-bit PNG) to an 8-bit sRGB JPEG with an
// embedded ICC profile, unifying the cache format across every preview stage
// and format. Old PNG caches are rebuildable and simply ignored.
const HEIF_FULL_CACHE_VERSION: &str = "heif-source-jpeg-v2";
// v9 separates positively identified Sony camera JPEGs from decoded primary
// images and records the display-oriented SDR/sRGB presentation policy.
pub(crate) const HEIF_CACHE_VERSION: &str = "heif-native-preview-v9-oriented-srgb";

// -----------------------------------------------------------------------------
// Preview artifacts
// -----------------------------------------------------------------------------

pub(crate) fn preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    priority: DecodePriority,
    try_fast_jpeg: bool,
    allow_decode: bool,
) -> Result<PreviewResult, MediaError> {
    let total_started = Instant::now();
    let max_size = max_size.max(1);
    fs::create_dir_all(cache_dir)?;

    // Sony HIF files carry a tiny camera-rendered JPEG specifically for fast
    // browsing. Probe and serve it inside execution, before entering the global
    // HEVC gate. Planning therefore remains independent of source contents.
    if try_fast_jpeg {
        let embedded_key = preview_cache_key(path, HEIF_CACHE_VERSION, 160)?;
        let embedded_destination = cache_dir.join(format!("{embedded_key}.embedded.jpg"));
        if embedded_destination.is_file() {
            return preview_result(embedded_destination, PreviewKind::Embedded);
        }
        if let Ok(inspection) = sony::inspect(path, None)
            && let Some(image) = inspection.embedded_jpeg
        {
            let decode_ms = duration_ms(total_started);
            let write_started = Instant::now();
            write_bytes_atomically(&image.bytes, &embedded_destination)?;
            let write_ms = duration_ms(write_started);
            let mut result = PreviewResult {
                path: embedded_destination,
                width: image.width,
                height: image.height,
                kind: PreviewKind::Embedded,
                render_level: None,
                diagnostics: None,
            };
            result.diagnostics = Some(PreviewDiagnostics {
                backend: Some("Sony HIF embedded JPEG".into()),
                queue_wait_ms: Some(0),
                source_wait_ms: Some(0),
                decode_ms: Some(decode_ms),
                encode_ms: Some(0),
                cache_sync_ms: Some(write_ms),
                cache_commit_ms: Some(0),
                total_ms: Some(duration_ms(total_started)),
                fallback_reason: None,
            });
            return Ok(result);
        }
    }

    let cache_key = preview_cache_key(path, HEIF_CACHE_VERSION, max_size)?;
    let destination = cache_dir.join(format!("{cache_key}.decoded.jpg"));
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }
    if let Some(result) =
        larger_cached_decoded_preview(path, cache_dir, HEIF_CACHE_VERSION, max_size)?
    {
        return Ok(result);
    }
    if !allow_decode {
        return Err(MediaError::NativeDecoderUnavailable);
    }

    let queue_started = Instant::now();
    let decode_permit = acquire_decode(priority);
    let queue_wait_ms = duration_ms(queue_started);
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }
    if let Some(result) =
        larger_cached_decoded_preview(path, cache_dir, HEIF_CACHE_VERSION, max_size)?
    {
        return Ok(result);
    }

    let source_lock_key = preview_cache_key(path, "heif-source-decode", 0)?;
    let source_wait_started = Instant::now();
    let (_heif_lock_arc, _decode_guard) = acquire_file_lock(&source_lock_key);
    let source_wait_ms = duration_ms(source_wait_started);
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }
    if let Some(result) =
        larger_cached_decoded_preview(path, cache_dir, HEIF_CACHE_VERSION, max_size)?
    {
        return Ok(result);
    }

    // Reuse full-detail cache if available to avoid redundant decoding.
    let cached_full_path = full_cache_path(path, cache_dir)?;
    let decode_started = Instant::now();
    let (image, backend, fallback_reason) = if cached_full_path.is_file() {
        (
            image::ImageReader::open(&cached_full_path)?
                .decode()?
                .thumbnail(max_size, max_size),
            "full-detail-cache",
            None,
        )
    } else {
        decode_preview(path, max_size)?
    };
    let decode_ms = duration_ms(decode_started);
    // The global gate protects scarce source decoding, not JPEG compression or
    // disk I/O. Releasing it here lets the selected full-resolution session and
    // visible thumbnails progress while this preview is being encoded.
    drop(decode_permit);
    let cache_write = write_jpeg_atomically_timed(&image, &destination, 90, HEIF_DECODED_JPEG)?;
    let mut result = preview_result(destination, PreviewKind::Decoded)?;
    result.diagnostics = Some(PreviewDiagnostics {
        backend: Some(backend.into()),
        queue_wait_ms: Some(queue_wait_ms),
        source_wait_ms: Some(source_wait_ms),
        decode_ms: Some(decode_ms),
        encode_ms: Some(cache_write.encode_ms),
        cache_sync_ms: Some(cache_write.sync_ms),
        cache_commit_ms: Some(cache_write.commit_ms),
        total_ms: Some(duration_ms(total_started)),
        fallback_reason,
    });
    Ok(result)
}

fn decode_preview(
    path: &Path,
    max_size: u32,
) -> Result<(DynamicImage, &'static str, Option<String>), MediaError> {
    let plan = backend_plan(path, HeifOperation::Preview);
    let result = execute_backend_plan(
        &plan,
        || false,
        |backend| match backend {
            #[cfg(target_os = "macos")]
            PlannedHeifBackend::Platform(_) => apple_image_io::decode_rgba8(path, max_size),
            #[cfg(target_os = "windows")]
            PlannedHeifBackend::Platform(_) => windows_wic::decode_full_rgba8(path)
                .map(|image| image.thumbnail(max_size, max_size)),
            #[cfg(target_os = "linux")]
            PlannedHeifBackend::Platform(_) => Err(MediaError::NativeDecoderUnavailable),
            PlannedHeifBackend::Ffmpeg | PlannedHeifBackend::FfmpegRgbaFallback => {
                ffmpeg_heif::decode_scaled_preview(path, max_size)
            }
            PlannedHeifBackend::Libheif => libheif::decode_scaled(path, max_size),
        },
    )
    .map_err(BackendExecutionError::into_media_error)?;
    let backend = match result.backend {
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
    Ok((
        result.value,
        backend,
        format_attempt_diagnostics(&result.diagnostics),
    ))
}

// -----------------------------------------------------------------------------
// Full-resolution artifact
// -----------------------------------------------------------------------------

pub(crate) fn full(path: &Path, cache_dir: &Path) -> Result<PreviewResult, MediaError> {
    fs::create_dir_all(cache_dir)?;
    let destination = full_cache_path(path, cache_dir)?;
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }

    let _decode_permit = acquire_decode(DecodePriority::Foreground);
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }

    let source_lock_key = preview_cache_key(path, "heif-source-decode", 0)?;
    let (_heif_lock_arc, _decode_guard) = acquire_file_lock(&source_lock_key);
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }

    let temporary = tempfile::Builder::new()
        .suffix(".jpg")
        .tempfile_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    let (backend, fallback_reason) = transcode_heif_source(path, temporary.path(), 95)?;
    persist_atomically(temporary, &destination)?;
    let mut result = preview_result(destination, PreviewKind::Decoded)?;
    result.diagnostics = Some(PreviewDiagnostics {
        backend: Some(backend.into()),
        fallback_reason,
        ..PreviewDiagnostics::default()
    });
    Ok(result)
}

/// Looks up a full-resolution JPEG previously converted from the source HEIF.
/// The Tauri HEIF session path uses this lookup to publish a completed full
/// artifact without starting another source decode.
pub fn cached_heif_full(
    path: &Path,
    cache_dir: &Path,
) -> Result<Option<PreviewResult>, MediaError> {
    let destination = full_cache_path(path, cache_dir)?;
    destination
        .is_file()
        .then(|| preview_result(destination, PreviewKind::Decoded))
        .transpose()
}

pub(crate) fn cache_full(path: &Path, cache_dir: &Path) -> Result<PathBuf, MediaError> {
    fs::create_dir_all(cache_dir)?;
    let destination = full_cache_path(path, cache_dir)?;
    if !destination.is_file() {
        let temporary = tempfile::Builder::new()
            .suffix(".jpg")
            .tempfile_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
        transcode_heif_source(path, temporary.path(), 95)?;
        persist_atomically(temporary, &destination)?;
    }
    Ok(destination)
}

fn full_cache_path(path: &Path, cache_dir: &Path) -> Result<PathBuf, MediaError> {
    let cache_key = preview_cache_key(path, HEIF_FULL_CACHE_VERSION, 0)?;
    Ok(cache_dir.join(format!("{cache_key}.jpg")))
}

fn transcode_heif_source(
    source: &Path,
    destination: &Path,
    quality: u8,
) -> Result<(&'static str, Option<String>), MediaError> {
    let plan = backend_plan(source, HeifOperation::FullArtifact);
    let result = execute_backend_plan(
        &plan,
        || false,
        |backend| {
            // A failed encoder may leave a header-only JPEG. Every fallback
            // starts from an empty destination and only the caller commits it.
            OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(destination)?;
            match backend {
                #[cfg(target_os = "macos")]
                PlannedHeifBackend::Platform(_) => {
                    apple_image_io::transcode_jpeg(source, destination, quality)
                }
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                PlannedHeifBackend::Platform(_) => Err(MediaError::NativeDecoderUnavailable),
                PlannedHeifBackend::Ffmpeg | PlannedHeifBackend::FfmpegRgbaFallback => {
                    ffmpeg_heif::can_decode(source)?;
                    ffmpeg_heif::transcode_full_jpeg(
                        source,
                        destination,
                        libheif::dimensions(source)?,
                        quality,
                    )
                }
                PlannedHeifBackend::Libheif => Err(MediaError::NativeDecoderUnavailable),
            }
        },
    )
    .map_err(BackendExecutionError::into_media_error)?;
    let backend = match result.backend {
        PlannedHeifBackend::Platform(oxy_domain::HeifBackendKind::AppleImageIo) => "Apple ImageIO",
        PlannedHeifBackend::Platform(_) => "platform HEIF backend",
        PlannedHeifBackend::Ffmpeg | PlannedHeifBackend::FfmpegRgbaFallback => "FFmpeg",
        PlannedHeifBackend::Libheif => "libheif",
    };
    Ok((backend, format_attempt_diagnostics(&result.diagnostics)))
}
