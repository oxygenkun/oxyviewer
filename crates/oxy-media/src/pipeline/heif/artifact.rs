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
        cache_tempfile, persist_atomically, preview_cache_key, write_bytes_atomically,
        write_jpeg_atomically_timed,
    },
    decode_control::{DecodePriority, acquire_decode, acquire_file_lock, file_lock},
    formats::heif::quirks::sony,
    media_source::{cached_preview_result, preview_result},
    policy::{HEIF_FULL, HEIF_PREVIEW},
    presentation::HEIF_DECODED_JPEG,
};
use image::DynamicImage;
use oxy_domain::{PreviewDiagnostics, PreviewKind, PreviewResult, RenderLevel};
use oxy_runtime::CancellationToken;
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::Instant,
};

/// Chooses the presentation boundary for HEIF full detail. The frontend does
/// not maintain a second platform policy table.
pub fn full_uses_artifact(
    path: &Path,
    cache_dir: &Path,
    display_sharpening: bool,
) -> Result<bool, MediaError> {
    if display_sharpening {
        return Ok(false);
    }
    if cached_heif_full(path, cache_dir)?.is_some() {
        return Ok(true);
    }
    #[cfg(target_os = "macos")]
    return Ok(true);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    Ok(false)
}

// Bumped from `libheif-1.23-sdr-v1` (16-bit PNG) to an 8-bit sRGB JPEG with an
// embedded ICC profile, unifying the cache format across every preview stage
// and format. Old PNG caches are rebuildable and simply ignored.
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
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let total_started = Instant::now();
    let max_size = max_size.max(1);
    let level = if max_size <= 512 {
        RenderLevel::Thumbnail
    } else {
        RenderLevel::Preview
    };
    fs::create_dir_all(cache_dir)?;

    // Sony HIF files carry a tiny camera-rendered JPEG specifically for fast
    // browsing. Probe and serve it inside execution, before entering the global
    // HEVC gate. Planning therefore remains independent of source contents.
    if try_fast_jpeg {
        let embedded_key = preview_cache_key(path, HEIF_PREVIEW, 160)?;
        let embedded_destination = cache_dir.join(format!("{embedded_key}.embedded.jpg"));
        if let Some(result) =
            cached_preview_result(embedded_destination.clone(), PreviewKind::Embedded, level)?
        {
            return Ok(result);
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
                render_level: level,
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

    let cache_key = preview_cache_key(path, HEIF_PREVIEW, max_size)?;
    let destination = cache_dir.join(format!("{cache_key}.decoded.jpg"));
    if let Some(result) = cached_preview_result(destination.clone(), PreviewKind::Decoded, level)? {
        return Ok(result);
    }
    if let Some(result) =
        larger_cached_decoded_preview(path, cache_dir, HEIF_PREVIEW, max_size, level)?
    {
        return Ok(result);
    }
    if !allow_decode {
        return Err(MediaError::NativeDecoderUnavailable);
    }

    let queue_started = Instant::now();
    let decode_permit = acquire_decode(priority, &|| cancellation.is_cancelled())?;
    let queue_wait_ms = duration_ms(queue_started);
    if let Some(result) = cached_preview_result(destination.clone(), PreviewKind::Decoded, level)? {
        return Ok(result);
    }
    if let Some(result) =
        larger_cached_decoded_preview(path, cache_dir, HEIF_PREVIEW, max_size, level)?
    {
        return Ok(result);
    }

    let source_lock_key = preview_cache_key(path, "heif-source-decode", 0)?;
    let source_wait_started = Instant::now();
    let source_lock = file_lock(&source_lock_key);
    let _decode_guard = acquire_file_lock(&source_lock, &|| cancellation.is_cancelled())?;
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let source_wait_ms = duration_ms(source_wait_started);
    if let Some(result) = cached_preview_result(destination.clone(), PreviewKind::Decoded, level)? {
        return Ok(result);
    }
    if let Some(result) =
        larger_cached_decoded_preview(path, cache_dir, HEIF_PREVIEW, max_size, level)?
    {
        return Ok(result);
    }

    // Reuse full-detail cache if available to avoid redundant decoding.
    let cached_full_path = full_cache_path(path, cache_dir)?;
    let decode_started = Instant::now();
    let cached_full = cached_preview_result(
        cached_full_path.clone(),
        PreviewKind::Decoded,
        RenderLevel::Full,
    )?
    .is_some();
    let (image, backend, fallback_reason) = if cached_full {
        (
            image::ImageReader::open(&cached_full_path)?
                .decode()?
                .thumbnail(max_size, max_size),
            "full-detail-cache",
            None,
        )
    } else {
        decode_preview(path, max_size, cancellation)?
    };
    let decode_ms = duration_ms(decode_started);
    // The global gate protects scarce source decoding, not JPEG compression or
    // disk I/O. Releasing it here lets the selected full-resolution session and
    // visible thumbnails progress while this preview is being encoded.
    drop(decode_permit);
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let cache_write =
        write_jpeg_atomically_timed(&image, &destination, 90, HEIF_DECODED_JPEG, || {
            cancellation.is_cancelled()
        })?;
    let mut result = preview_result(destination, PreviewKind::Decoded, level)?;
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
    cancellation: &CancellationToken,
) -> Result<(DynamicImage, &'static str, Option<String>), MediaError> {
    let plan = backend_plan(path, HeifOperation::Preview);
    let result = execute_backend_plan(
        &plan,
        || cancellation.is_cancelled(),
        |backend| match backend {
            PlannedHeifBackend::CachedArtifact => Err(MediaError::NativeDecoderUnavailable),
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
    Ok((
        result.value,
        backend,
        format_attempt_diagnostics(&result.diagnostics),
    ))
}

// -----------------------------------------------------------------------------
// Full-resolution artifact
// -----------------------------------------------------------------------------

pub(crate) fn full(
    path: &Path,
    cache_dir: &Path,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    fs::create_dir_all(cache_dir)?;
    let destination = full_cache_path(path, cache_dir)?;
    if let Some(result) =
        cached_preview_result(destination.clone(), PreviewKind::Decoded, RenderLevel::Full)?
    {
        return Ok(result);
    }

    let _decode_permit =
        acquire_decode(DecodePriority::Foreground, &|| cancellation.is_cancelled())?;
    if let Some(result) =
        cached_preview_result(destination.clone(), PreviewKind::Decoded, RenderLevel::Full)?
    {
        return Ok(result);
    }

    let source_lock_key = preview_cache_key(path, "heif-source-decode", 0)?;
    let source_lock = file_lock(&source_lock_key);
    let _decode_guard = acquire_file_lock(&source_lock, &|| cancellation.is_cancelled())?;
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    if let Some(result) =
        cached_preview_result(destination.clone(), PreviewKind::Decoded, RenderLevel::Full)?
    {
        return Ok(result);
    }

    let temporary = cache_tempfile(&destination, ".jpg")?;
    let (backend, fallback_reason) =
        transcode_heif_source(path, temporary.path(), 95, || cancellation.is_cancelled())?;
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    persist_atomically(temporary, &destination)?;
    let mut result = preview_result(destination, PreviewKind::Decoded, RenderLevel::Full)?;
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
    cached_preview_result(destination, PreviewKind::Decoded, RenderLevel::Full)
}

pub(crate) fn cache_full(path: &Path, cache_dir: &Path) -> Result<PathBuf, MediaError> {
    fs::create_dir_all(cache_dir)?;
    let destination = full_cache_path(path, cache_dir)?;
    if cached_preview_result(destination.clone(), PreviewKind::Decoded, RenderLevel::Full)?
        .is_none()
    {
        let temporary = cache_tempfile(&destination, ".jpg")?;
        transcode_heif_source(path, temporary.path(), 95, || false)?;
        persist_atomically(temporary, &destination)?;
    }
    Ok(destination)
}

fn full_cache_path(path: &Path, cache_dir: &Path) -> Result<PathBuf, MediaError> {
    let cache_key = preview_cache_key(path, HEIF_FULL, 0)?;
    Ok(cache_dir.join(format!("{cache_key}.jpg")))
}

#[cfg(test)]
mod delivery_tests {
    use super::*;

    #[test]
    fn sharpening_always_uses_tiles_without_touching_the_canonical_cache() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.heif");
        std::fs::write(&source, b"cache identity").unwrap();

        assert!(!full_uses_artifact(&source, directory.path(), true).unwrap());
    }

    #[test]
    fn a_valid_canonical_full_cache_is_delivered_as_an_artifact() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.heif");
        std::fs::write(&source, b"cache identity").unwrap();
        let destination = full_cache_path(&source, directory.path()).unwrap();
        image::DynamicImage::new_rgb8(32, 16)
            .save(&destination)
            .unwrap();

        assert!(full_uses_artifact(&source, directory.path(), false).unwrap());
    }
}

fn transcode_heif_source(
    source: &Path,
    destination: &Path,
    quality: u8,
    cancelled: impl FnMut() -> bool,
) -> Result<(&'static str, Option<String>), MediaError> {
    let plan = backend_plan(source, HeifOperation::FullArtifact);
    let result = execute_backend_plan(&plan, cancelled, |backend| {
        // A failed encoder may leave a header-only JPEG. Every fallback
        // starts from an empty destination and only the caller commits it.
        OpenOptions::new()
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
    })
    .map_err(BackendExecutionError::into_media_error)?;
    let backend = match result.backend {
        PlannedHeifBackend::CachedArtifact => "cached full JPEG",
        PlannedHeifBackend::Platform(oxy_domain::HeifBackendKind::AppleImageIo) => "Apple ImageIO",
        PlannedHeifBackend::Platform(_) => "platform HEIF backend",
        PlannedHeifBackend::Ffmpeg | PlannedHeifBackend::FfmpegRgbaFallback => "FFmpeg",
        PlannedHeifBackend::Libheif => "libheif",
    };
    Ok((backend, format_attempt_diagnostics(&result.diagnostics)))
}
