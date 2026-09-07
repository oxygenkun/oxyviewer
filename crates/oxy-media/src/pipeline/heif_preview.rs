use super::{
    artifact::{PREVIEW_CACHE_SIZES, duration_ms},
    heif::{
        BackendExecutionError, HeifBackend as PlannedHeifBackend, HeifOperation, backend_plan,
        execute_backend_plan, format_attempt_diagnostics,
    },
    planner::{Presence, SourceFacts, Vendor},
};
#[cfg(target_os = "macos")]
use crate::backends::apple_image_io;
#[cfg(target_os = "windows")]
use crate::backends::windows_wic;
use crate::{
    HeifDecodePriority, MediaError,
    backends::{ffmpeg_heif, libheif},
    cache::{
        persist_atomically, preview_cache_key, write_bytes_atomically, write_jpeg_atomically_timed,
    },
    decode_control::{acquire_file_lock, acquire_heif_decode},
    formats::heif::quirks::sony,
    media_source::preview_result,
    presentation::HEIF_DECODED_JPEG,
    probe::{self, ProbedSource},
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

pub(crate) fn heif_full(path: &Path, cache_dir: &Path) -> Result<PreviewResult, MediaError> {
    fs::create_dir_all(cache_dir)?;
    let cache_key = preview_cache_key(path, HEIF_FULL_CACHE_VERSION, 0)?;
    let destination = cache_dir.join(format!("{cache_key}.jpg"));
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }

    let _decode_permit = acquire_heif_decode(HeifDecodePriority::Foreground);
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

fn heif_session_cache_path(path: &Path, cache_dir: &Path) -> Result<PathBuf, MediaError> {
    let cache_key = preview_cache_key(path, HEIF_FULL_CACHE_VERSION, 0)?;
    Ok(cache_dir.join(format!("{cache_key}.jpg")))
}

/// Looks up a full-resolution JPEG previously converted from the source HEIF.
/// The Tauri HEIF session path uses this lookup to publish a completed full
/// artifact without starting another source decode.
pub fn cached_heif_session(
    path: &Path,
    cache_dir: &Path,
) -> Result<Option<PreviewResult>, MediaError> {
    let destination = heif_session_cache_path(path, cache_dir)?;
    destination
        .is_file()
        .then(|| preview_result(destination, PreviewKind::Decoded))
        .transpose()
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

pub(crate) fn cache_heif_source_jpeg(path: &Path, cache_dir: &Path) -> Result<PathBuf, MediaError> {
    fs::create_dir_all(cache_dir)?;
    let destination = heif_session_cache_path(path, cache_dir)?;
    if !destination.is_file() {
        let temporary = tempfile::Builder::new()
            .suffix(".jpg")
            .tempfile_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
        transcode_heif_source(path, temporary.path(), 95)?;
        persist_atomically(temporary, &destination)?;
    }
    Ok(destination)
}

pub fn heif_preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
) -> Result<PreviewResult, MediaError> {
    heif_preview_with_priority(path, cache_dir, max_size, HeifDecodePriority::Background)
}

pub(crate) fn probe_heif_for_preview(path: &Path, cache_dir: &Path) -> ProbedSource {
    let cached_fast_representation = preview_cache_key(path, HEIF_CACHE_VERSION, 160)
        .ok()
        .map(|key| cache_dir.join(format!("{key}.embedded.jpg")))
        .is_some_and(|path| path.is_file());
    if cached_fast_representation {
        ProbedSource {
            facts: SourceFacts {
                kind: oxy_domain::AssetKind::Heif,
                vendor: Vendor::Sony,
                heif_fast_jpeg: Presence::Present,
            },
            heif_fast_jpeg: None,
        }
    } else {
        probe::heif(path)
    }
}

pub(crate) fn heif_preview_with_priority(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    priority: HeifDecodePriority,
) -> Result<PreviewResult, MediaError> {
    let probed = (max_size <= 160).then(|| probe_heif_for_preview(path, cache_dir));
    let try_fast_jpeg = probed
        .as_ref()
        .is_some_and(|source| source.facts.heif_fast_jpeg == Presence::Present);
    let fast_jpeg = probed
        .as_ref()
        .and_then(|source| source.heif_fast_jpeg.as_ref());
    heif_preview_with_options(
        path,
        cache_dir,
        max_size,
        priority,
        try_fast_jpeg,
        true,
        fast_jpeg,
    )
}

pub(crate) fn heif_preview_with_options(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    priority: HeifDecodePriority,
    try_fast_jpeg: bool,
    allow_decode: bool,
    fast_jpeg: Option<&sony::EmbeddedJpeg>,
) -> Result<PreviewResult, MediaError> {
    let total_started = Instant::now();
    let max_size = max_size.max(1);
    fs::create_dir_all(cache_dir)?;
    let cache_key = preview_cache_key(path, HEIF_CACHE_VERSION, max_size)?;
    let embedded_destination = cache_dir.join(format!("{cache_key}.embedded.jpg"));
    let destination = cache_dir.join(format!("{cache_key}.decoded.jpg"));
    if try_fast_jpeg && embedded_destination.is_file() {
        return preview_result(embedded_destination, PreviewKind::Embedded);
    }
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }
    if let Some(result) = larger_cached_preview(path, cache_dir, HEIF_CACHE_VERSION, max_size)? {
        return Ok(result);
    }

    // Sony HIF files carry a tiny camera-rendered JPEG specifically for fast
    // browsing. Serve it before entering the global HEVC gate: a scrolling
    // viewport must not launch FFmpeg or wait behind full-image decode merely
    // to paint a 160 px placeholder.
    if try_fast_jpeg
        && max_size <= 160
        && let Some(image) = fast_jpeg
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
    if !allow_decode {
        return Err(MediaError::NativeDecoderUnavailable);
    }

    let queue_started = Instant::now();
    let decode_permit = acquire_heif_decode(priority);
    let queue_wait_ms = duration_ms(queue_started);
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }
    if let Some(result) = larger_cached_preview(path, cache_dir, HEIF_CACHE_VERSION, max_size)? {
        return Ok(result);
    }

    let source_lock_key = preview_cache_key(path, "heif-source-decode", 0)?;
    let source_wait_started = Instant::now();
    let (_heif_lock_arc, _decode_guard) = acquire_file_lock(&source_lock_key);
    let source_wait_ms = duration_ms(source_wait_started);
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }
    if let Some(result) = larger_cached_preview(path, cache_dir, HEIF_CACHE_VERSION, max_size)? {
        return Ok(result);
    }

    // Reuse full-detail cache if available to avoid redundant decoding.
    let full_cache_key = preview_cache_key(path, HEIF_FULL_CACHE_VERSION, 0)?;
    let full_cache_path = cache_dir.join(format!("{full_cache_key}.jpg"));
    let decode_started = Instant::now();
    let (image, backend, fallback_reason) = if full_cache_path.is_file() {
        (
            image::ImageReader::open(&full_cache_path)?
                .decode()?
                .thumbnail(max_size, max_size),
            "full-detail-cache",
            None,
        )
    } else {
        decode_heif_preview(path, max_size)?
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

fn decode_heif_preview(
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

/// Satisfy a request for `max_size` from any larger cached entry produced by
/// the same backend. This generalizes the former HEIF-only
/// `larger_heif_preview` to every format: a 512 px request can be served from a
/// cached 4096 px entry or the full-resolution entry without re-decoding. The
/// browser downscales the returned JPEG via CSS, so no backend resize is
/// needed.
///
/// `backend_tag` is the same cache-version string used for writes (e.g.
/// [`HEIF_CACHE_VERSION`], [`LIBRAW_CACHE_VERSION`]). Candidates are scanned
/// smallest-first so the closest larger entry wins, minimizing transfer size.
pub(crate) fn larger_cached_preview(
    path: &Path,
    cache_dir: &Path,
    backend_tag: &str,
    max_size: u32,
) -> Result<Option<PreviewResult>, MediaError> {
    let mut candidates: Vec<u32> = PREVIEW_CACHE_SIZES
        .iter()
        .copied()
        .filter(|&size| size > max_size)
        .collect();
    candidates.sort();
    for candidate_size in candidates {
        let key = preview_cache_key(path, backend_tag, candidate_size)?;
        let candidate = cache_dir.join(format!("{key}.decoded.jpg"));
        if candidate.is_file() {
            return preview_result(candidate, PreviewKind::Decoded).map(Some);
        }
    }
    Ok(None)
}
