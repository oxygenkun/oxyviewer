#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
compile_error!("oxy-media supports only Windows, macOS, and Linux");

mod backends;
mod cache;
mod decode_control;
mod formats;
mod heif_service;
mod pipeline;

#[cfg(target_os = "macos")]
use backends::apple_image_io;
#[cfg(target_os = "windows")]
use backends::windows_wic;
use backends::{ffmpeg_heif, libheif, libraw};
use formats::heif::quirks::sony;
use pipeline::{
    heif::{
        BackendExecutionError, HeifBackend as PlannedHeifBackend, HeifOperation, backend_plan,
        execute_backend_plan, format_attempt_diagnostics,
    },
    planner::{
        BackendCapabilities, DecodePlan, DecodeStep, PlannedPriority, Platform, Request,
        SourceFacts, plan,
    },
};

pub use cache::{CacheUsage, clear_preview_cache, preview_cache_usage, prune_preview_cache};
use cache::{
    persist_atomically, preview_cache_key, write_bytes_atomically, write_jpeg_atomically,
    write_jpeg_atomically_timed,
};
pub use decode_control::{DecodePriority, HeifDecodePriority};
pub(crate) use decode_control::{
    acquire_decode, acquire_heif_decode, try_acquire_heif_session_cache_write,
};
use decode_control::{acquire_file_lock, acquire_raw_full_decode};
pub use heif_service::{DEFAULT_TILE_SIZE, HeifBackend, HeifDecodeService, HeifTile, TileSink};
use image::{DynamicImage, ImageReader};
use oxy_domain::{PreviewDiagnostics, PreviewKind, PreviewResult, RenderLevel};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::{
    fs::{self, File, OpenOptions},
    io::BufReader,
    path::{Path, PathBuf},
    time::Instant,
};
use tempfile::NamedTempFile;
use thiserror::Error;

const LIBRAW_CACHE_VERSION: &str = "libraw-0.22.2-v6";
const LIBRAW_FULL_CACHE_VERSION: &str = "libraw-0.22.2-full-detail-v2";
// Bumped from `libheif-1.23-sdr-v1` (16-bit PNG) to an 8-bit sRGB JPEG with an
// embedded ICC profile, unifying the cache format across every preview stage
// and format. Old PNG caches are rebuildable and simply ignored.
const HEIF_FULL_CACHE_VERSION: &str = "heif-source-jpeg-v2";
// v8 corrects the direction of Sony HIF embedded-JPEG portrait rotation.
const HEIF_CACHE_VERSION: &str = "heif-native-preview-v8";
const SYSTEM_CACHE_VERSION: &str = "system-preview-v2";
/// Cache sizes shared by every format's progressive pipeline. A request for a
/// smaller size may be satisfied by any larger cached entry (see
/// [`larger_cached_preview`]).
const PREVIEW_CACHE_SIZES: [u32; 2] = [512, 4_096];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("format requires a native decoder that is not available")]
    NativeDecoderUnavailable,
    #[error("{backend} native decode failed: {message}")]
    NativeDecode {
        backend: &'static str,
        message: String,
    },
    #[error("LibRaw failed for {path}: {message}")]
    LibRaw { path: PathBuf, message: String },
    #[error("libheif failed for {path}: {message}")]
    Heif { path: PathBuf, message: String },
    #[error("color conversion failed: {0}")]
    Color(String),
    #[error("decode session was cancelled")]
    Cancelled,
    #[error("all backend attempts failed ({attempts}): {source}")]
    BackendAttempts {
        attempts: String,
        #[source]
        source: Box<MediaError>,
    },
    #[error("system preview generation failed for {path}: {message}")]
    PreviewGenerationFailed { path: PathBuf, message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
}

pub fn dimensions(path: &Path) -> Result<ImageDimensions, MediaError> {
    let raster = || -> Result<ImageDimensions, MediaError> {
        let reader = ImageReader::new(BufReader::new(File::open(path)?)).with_guessed_format()?;
        let (width, height) = reader.into_dimensions()?;
        Ok(ImageDimensions { width, height })
    };

    raster()
        .or_else(|_| libheif::dimensions(path))
        .or_else(|_| raw_dimensions(path))
}

pub fn raw_dimensions(path: &Path) -> Result<ImageDimensions, MediaError> {
    libraw::dimensions(path).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    })
}

pub fn raw_preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
) -> Result<PreviewResult, MediaError> {
    raw_preview_with_priority(path, cache_dir, max_size, DecodePriority::Background)
}

pub fn raw_preview_with_priority(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    priority: DecodePriority,
) -> Result<PreviewResult, MediaError> {
    let max_size = max_size.max(1);
    fs::create_dir_all(cache_dir)?;
    let cache_key = preview_cache_key(path, LIBRAW_CACHE_VERSION, max_size)?;
    for (suffix, kind) in [
        ("embedded.jpg", PreviewKind::Embedded),
        ("developed.jpg", PreviewKind::Developed),
    ] {
        let destination = cache_dir.join(format!("{cache_key}.{suffix}"));
        if destination.is_file() {
            return preview_result(destination, kind);
        }
    }
    // Up-tier reuse: a larger cached RAW preview can satisfy this request
    // without re-decoding. Mirrors the HEIF path's behavior.
    if let Some(result) = larger_cached_raw_preview(path, cache_dir, max_size)? {
        return Ok(result);
    }

    // Unified decode gate: visible/loupe thumbnails jump ahead of background
    // overscan. Per-file lock coalesces duplicate work for the same source.
    let _decode_permit = acquire_decode(priority);
    for (suffix, kind) in [
        ("embedded.jpg", PreviewKind::Embedded),
        ("developed.jpg", PreviewKind::Developed),
    ] {
        let destination = cache_dir.join(format!("{cache_key}.{suffix}"));
        if destination.is_file() {
            return preview_result(destination, kind);
        }
    }
    if let Some(result) = larger_cached_raw_preview(path, cache_dir, max_size)? {
        return Ok(result);
    }

    let source_lock_key = preview_cache_key(path, "raw-source-decode", 0)?;
    let (_lock_arc, _decode_guard) = acquire_file_lock(&source_lock_key);
    for (suffix, kind) in [
        ("embedded.jpg", PreviewKind::Embedded),
        ("developed.jpg", PreviewKind::Developed),
    ] {
        let destination = cache_dir.join(format!("{cache_key}.{suffix}"));
        if destination.is_file() {
            return preview_result(destination, kind);
        }
    }
    if let Some(result) = larger_cached_raw_preview(path, cache_dir, max_size)? {
        return Ok(result);
    }

    // Preserve the size-selected camera JPEG at every semantic level. Decoding,
    // resizing, and re-encoding it would serialize a cold ARW grid for no
    // visual benefit; the WebView can scale the artifact like the HEIF path.
    let preview = libraw::preview(path, max_size, true).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    })?;
    let (destination, kind) = match preview {
        libraw::Preview::EmbeddedJpeg(data) => {
            let destination = cache_dir.join(format!("{cache_key}.embedded.jpg"));
            write_bytes_atomically(&data, &destination)?;
            (destination, PreviewKind::Embedded)
        }
        libraw::Preview::Image(image) => {
            let destination = cache_dir.join(format!("{cache_key}.developed.jpg"));
            write_jpeg_atomically(&image, &destination, 90)?;
            (destination, PreviewKind::Developed)
        }
    };
    preview_result(destination, kind)
}

pub fn raw_full(path: &Path, cache_dir: &Path) -> Result<PreviewResult, MediaError> {
    fs::create_dir_all(cache_dir)?;
    let destination = cache_dir.join(format!(
        "{}.jpg",
        preview_cache_key(path, LIBRAW_FULL_CACHE_VERSION, 0)?
    ));

    // Sony ARW files normally contain a camera-rendered JPEG at effectively
    // the sensor's full resolution. Fast photo viewers use it for immediate
    // 1:1 inspection. Reuse the loupe cache (or extract it in milliseconds)
    // instead of demosaicing the frame while the user zooms and pans.
    let source_size = raw_dimensions(path)?;
    let embedded = raw_preview_with_priority(path, cache_dir, 4_096, DecodePriority::Foreground)?;
    if embedded.kind == PreviewKind::Embedded
        && covers_raw_source(
            ImageDimensions {
                width: embedded.width,
                height: embedded.height,
            },
            source_size,
        )
    {
        return Ok(embedded);
    }

    // Only consult an older developed cache when the embedded image cannot
    // provide near-full detail. This ordering also upgrades existing installs:
    // a large, slow-to-load full JPEG from an earlier version no longer masks
    // the much smaller camera JPEG fast path.
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Developed);
    }

    let _decode_guard = acquire_raw_full_decode();
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Developed);
    }

    let image = libraw::full(path).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    })?;
    let image = image.unsharpen(0.8, 2);
    write_jpeg_atomically(&image, &destination, 95)?;
    preview_result(destination, PreviewKind::Developed)
}

fn covers_raw_source(candidate: ImageDimensions, source: ImageDimensions) -> bool {
    let mut candidate_edges = [candidate.width, candidate.height];
    let mut source_edges = [source.width, source.height];
    candidate_edges.sort_unstable();
    source_edges.sort_unstable();

    // Allow the small active-area/crop difference between LibRaw's dimensions
    // and the camera JPEG. A genuinely reduced preview still falls through.
    candidate_edges
        .into_iter()
        .zip(source_edges)
        .all(|(candidate, source)| u64::from(candidate) * 100 >= u64::from(source) * 90)
}

pub fn heif_full(path: &Path, cache_dir: &Path) -> Result<PreviewResult, MediaError> {
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

/// Returns a full-resolution JPEG previously converted from the source HEIF.
/// Kept for the disabled tile-session compatibility entry point; the active
/// loupe requests the same artifact through [`heif_full`].
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
                    crate::apple_image_io::transcode_jpeg(source, destination, quality)
                }
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                PlannedHeifBackend::Platform(_) => Err(MediaError::NativeDecoderUnavailable),
                PlannedHeifBackend::Ffmpeg | PlannedHeifBackend::FfmpegRgbaFallback => {
                    crate::ffmpeg_heif::can_decode(source)?;
                    crate::ffmpeg_heif::transcode_full_jpeg(
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

pub fn heif_preview_with_priority(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    priority: HeifDecodePriority,
) -> Result<PreviewResult, MediaError> {
    heif_preview_with_options(path, cache_dir, max_size, priority, true, true)
}

fn heif_preview_with_options(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    priority: HeifDecodePriority,
    try_fast_jpeg: bool,
    allow_decode: bool,
) -> Result<PreviewResult, MediaError> {
    let total_started = Instant::now();
    let max_size = max_size.max(1);
    fs::create_dir_all(cache_dir)?;
    let cache_key = preview_cache_key(path, HEIF_CACHE_VERSION, max_size)?;
    let destination = cache_dir.join(format!("{cache_key}.jpg"));
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
        && let Ok(display_size) = libheif::dimensions(path)
        && let Ok(image) = sony::extract(path, display_size)
    {
        let decode_ms = duration_ms(total_started);
        let write_started = Instant::now();
        write_bytes_atomically(&image.bytes, &destination)?;
        let write_ms = duration_ms(write_started);
        let mut result = PreviewResult {
            path: destination,
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
    let cache_write = write_jpeg_atomically_timed(&image, &destination, 90)?;
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
            PlannedHeifBackend::Platform(_) => crate::apple_image_io::decode_rgba8(path, max_size),
            #[cfg(target_os = "windows")]
            PlannedHeifBackend::Platform(_) => crate::windows_wic::decode_full_rgba8(path)
                .map(|image| image.thumbnail(max_size, max_size)),
            #[cfg(target_os = "linux")]
            PlannedHeifBackend::Platform(_) => Err(MediaError::NativeDecoderUnavailable),
            PlannedHeifBackend::Ffmpeg | PlannedHeifBackend::FfmpegRgbaFallback => {
                crate::ffmpeg_heif::decode_scaled_preview(path, max_size)
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

fn duration_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
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
fn larger_cached_preview(
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
        let candidate = cache_dir.join(format!("{key}.jpg"));
        if candidate.is_file() {
            return preview_result(candidate, PreviewKind::Decoded).map(Some);
        }
    }
    Ok(None)
}

fn larger_cached_raw_preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
) -> Result<Option<PreviewResult>, MediaError> {
    for candidate_size in PREVIEW_CACHE_SIZES
        .iter()
        .copied()
        .filter(|&size| size > max_size)
    {
        let key = preview_cache_key(path, LIBRAW_CACHE_VERSION, candidate_size)?;
        for (suffix, kind) in [
            ("embedded.jpg", PreviewKind::Embedded),
            ("developed.jpg", PreviewKind::Developed),
        ] {
            let candidate = cache_dir.join(format!("{key}.{suffix}"));
            if candidate.is_file() {
                return preview_result(candidate, kind).map(Some);
            }
        }
    }
    Ok(None)
}

pub fn system_preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
) -> Result<PreviewResult, MediaError> {
    fs::create_dir_all(cache_dir)?;
    let destination = cache_dir.join(format!(
        "{}.png",
        preview_cache_key(path, SYSTEM_CACHE_VERSION, max_size)?
    ));
    if destination.is_file() {
        return preview_result(destination, PreviewKind::System);
    }
    generate_system_preview(path, &destination, max_size)?;
    preview_result(destination, PreviewKind::System)
}

pub fn original(path: PathBuf) -> Result<PreviewResult, MediaError> {
    preview_result(path, PreviewKind::Original)
}

/// Convert the IPC-level [`PreviewPriority`] into the unified
/// [`DecodePriority`]. Kept here (in oxy-media) so the Tauri layer does not
/// need to know about the gate vocabulary.
pub fn decode_priority_for(priority: oxy_domain::PreviewPriority) -> DecodePriority {
    use oxy_domain::PreviewPriority;
    match priority {
        PreviewPriority::Preload => DecodePriority::Background,
        PreviewPriority::Nearby => DecodePriority::Background,
        PreviewPriority::Visible => DecodePriority::Visible,
        PreviewPriority::Loupe => DecodePriority::Foreground,
    }
}

const fn current_platform() -> Platform {
    #[cfg(target_os = "windows")]
    {
        Platform::Windows
    }
    #[cfg(target_os = "macos")]
    {
        Platform::Macos
    }
    #[cfg(target_os = "linux")]
    {
        Platform::Linux
    }
}

/// Unified preview dispatcher. Callers request a semantic [`RenderLevel`];
/// this module alone chooses the platform/format-specific decoder and size.
pub fn preview(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    priority: DecodePriority,
    kind: oxy_domain::AssetKind,
) -> Result<PreviewResult, MediaError> {
    // Preserve the pre-planner executor's optimistic route set. Runtime
    // adapter failures still flow through the same ordered fallbacks and retain
    // their existing diagnostics; simulated capabilities are tested in the
    // pure planner without making directory discovery probe media contents.
    let decode_plan = plan(
        SourceFacts::unprobed(kind),
        Request {
            level,
            platform: current_platform(),
        },
        BackendCapabilities::configured_routes(),
    );
    let mut result = execute_decode_plan(path, cache_dir, priority, decode_plan)?;
    result.render_level = Some(level);
    Ok(result)
}

fn execute_decode_plan(
    path: &Path,
    cache_dir: &Path,
    request_priority: DecodePriority,
    decode_plan: DecodePlan,
) -> Result<PreviewResult, MediaError> {
    let DecodePlan::Attempts { first, on_failure } = decode_plan else {
        return Err(MediaError::NativeDecoderUnavailable);
    };
    match (first, on_failure) {
        (
            DecodeStep::RawPreview { max_size },
            Some(DecodeStep::SystemPreview {
                max_size: fallback_size,
            }),
        ) => match raw_preview_with_priority(path, cache_dir, max_size, request_priority) {
            result @ Ok(_) | result @ Err(MediaError::Cancelled) => result,
            Err(error) => {
                // Preserve the original LibRaw error when the system fallback
                // also fails so diagnostics remain actionable.
                match system_preview(path, cache_dir, fallback_size) {
                    Ok(result) => Ok(result),
                    Err(system_error) => Err(MediaError::LibRaw {
                        path: path.to_owned(),
                        message: format!("{error}; fallback failed: {system_error}"),
                    }),
                }
            }
        },
        (DecodeStep::HeifFull, Some(fallback)) => match heif_full(path, cache_dir) {
            result @ Ok(_) | result @ Err(MediaError::Cancelled) => result,
            Err(error) => {
                // Preserve the foreground 8192px fallback after full-detail
                // HEIF failure so the loupe is not left empty.
                eprintln!(
                    "full-detail HEIF decode failed for {}: {error}",
                    path.display()
                );
                execute_decode_step(path, cache_dir, request_priority, fallback)
            }
        },
        (first, None) => execute_decode_step(path, cache_dir, request_priority, first),
        // The Stage-B planner only emits the two ordered fallback pairs above.
        // Treat a future unsupported pairing explicitly rather than silently
        // changing its error or fallback behavior.
        (_, Some(_)) => Err(MediaError::NativeDecoderUnavailable),
    }
}

fn execute_decode_step(
    path: &Path,
    cache_dir: &Path,
    request_priority: DecodePriority,
    step: DecodeStep,
) -> Result<PreviewResult, MediaError> {
    match step {
        DecodeStep::Original => original(path.to_owned()),
        DecodeStep::RawPreview { max_size } => {
            raw_preview_with_priority(path, cache_dir, max_size, request_priority)
        }
        DecodeStep::RawFull => raw_full(path, cache_dir),
        DecodeStep::HeifPreview {
            max_size,
            try_fast_jpeg,
            allow_decode,
            priority,
        } => {
            let priority = match priority {
                PlannedPriority::Request => request_priority,
                PlannedPriority::Foreground => DecodePriority::Foreground,
            };
            heif_preview_with_options(
                path,
                cache_dir,
                max_size,
                priority,
                try_fast_jpeg,
                allow_decode,
            )
        }
        DecodeStep::HeifFull => heif_full(path, cache_dir),
        DecodeStep::SystemPreview { max_size } => system_preview(path, cache_dir, max_size),
    }
}

fn preview_result(path: PathBuf, kind: PreviewKind) -> Result<PreviewResult, MediaError> {
    let size = dimensions(&path)?;
    Ok(PreviewResult {
        path,
        width: size.width,
        height: size.height,
        kind,
        render_level: None,
        diagnostics: None,
    })
}

#[cfg(target_os = "macos")]
fn generate_system_preview(
    source: &Path,
    destination: &Path,
    max_size: u32,
) -> Result<(), MediaError> {
    let output = tempfile::tempdir()?;
    let result = Command::new("/usr/bin/qlmanage")
        .arg("-t")
        .arg("-s")
        .arg(max_size.to_string())
        .arg("-o")
        .arg(output.path())
        .arg(source)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if !result.status.success() {
        return Err(MediaError::PreviewGenerationFailed {
            path: source.to_owned(),
            message: String::from_utf8_lossy(&result.stderr).trim().to_owned(),
        });
    }
    let generated = fs::read_dir(output.path())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_file())
        .ok_or_else(|| MediaError::PreviewGenerationFailed {
            path: source.to_owned(),
            message: "Quick Look did not produce a preview".into(),
        })?;
    let temporary = NamedTempFile::new_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    fs::copy(generated, temporary.path())?;
    match temporary.persist_noclobber(destination) {
        Ok(_) => Ok(()),
        Err(_error) if destination.is_file() => Ok(()),
        Err(error) => Err(MediaError::Io(error.error)),
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn generate_system_preview(
    _source: &Path,
    _destination: &Path,
    _max_size: u32,
) -> Result<(), MediaError> {
    Err(MediaError::NativeDecoderUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Rgb, RgbImage};
    use std::{
        io::Cursor,
        time::{Duration, Instant},
    };

    fn workspace_path(path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        if path.is_absolute() {
            path.to_owned()
        } else {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(path)
        }
    }

    #[test]
    fn heif_session_cache_is_lookup_only_and_comes_from_the_source_heif() {
        let directory = tempfile::tempdir().unwrap();
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        let cache = directory.path().join("previews");

        assert!(cached_heif_session(&source, &cache).unwrap().is_none());

        cache_heif_source_jpeg(&source, &cache).unwrap();
        let cached = cached_heif_session(&source, &cache)
            .unwrap()
            .expect("source HEIF should have a full JPEG cache");
        assert!(cached.width >= 4_672);
        assert!(cached.height >= 4_672);
        assert!(cached.path.is_file());
    }

    #[test]
    fn sony_hif_thumbnail_and_preview_levels_share_the_160_artifact() {
        let path = workspace_path("tests/fixtures/DSC00449.HIF");
        let cache = tempfile::tempdir().unwrap();

        let thumbnail = preview(
            &path,
            cache.path(),
            RenderLevel::Thumbnail,
            DecodePriority::Visible,
            oxy_domain::AssetKind::Heif,
        )
        .unwrap();
        let loupe_base = preview(
            &path,
            cache.path(),
            RenderLevel::Preview,
            DecodePriority::Foreground,
            oxy_domain::AssetKind::Heif,
        )
        .unwrap();

        assert_eq!((thumbnail.width, thumbnail.height), (120, 160));
        assert_eq!(loupe_base.path, thumbnail.path);
        assert_eq!(thumbnail.render_level, Some(RenderLevel::Thumbnail));
        assert_eq!(loupe_base.render_level, Some(RenderLevel::Preview));
    }

    #[test]
    fn larger_cached_preview_satisfies_smaller_request() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.heic");
        fs::write(&path, b"heif").unwrap();
        let key = preview_cache_key(&path, HEIF_CACHE_VERSION, 4_096).unwrap();
        let cached = directory.path().join(format!("{key}.jpg"));
        write_jpeg_atomically(&DynamicImage::new_rgb8(32, 16), &cached, 90).unwrap();

        let result = larger_cached_preview(&path, directory.path(), HEIF_CACHE_VERSION, 512)
            .unwrap()
            .unwrap();

        assert_eq!(result.path, cached);
        assert_eq!((result.width, result.height), (32, 16));
    }

    #[test]
    fn larger_cached_preview_serves_any_backend_tag() {
        // The unified cache lookup must work for non-HEIF backends too. A
        // synthetic RAW-style tag with a 4096 entry should satisfy a 512
        // request without re-decoding.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.arw");
        fs::write(&path, b"raw").unwrap();
        let raw_tag = "libraw-0.22.2-v6";
        let key = preview_cache_key(&path, raw_tag, 4_096).unwrap();
        let cached = directory.path().join(format!("{key}.jpg"));
        write_jpeg_atomically(&DynamicImage::new_rgb8(48, 24), &cached, 90).unwrap();

        let result = larger_cached_preview(&path, directory.path(), raw_tag, 512)
            .unwrap()
            .unwrap();

        assert_eq!(result.path, cached);
    }

    #[test]
    fn heif_preview_reports_cold_backend_timing_breakdown() {
        let path = workspace_path("tests/fixtures/DSC00449.HIF");
        let cache = tempfile::tempdir().unwrap();

        let preview = heif_preview(&path, cache.path(), 512).unwrap();
        let diagnostics = preview.diagnostics.expect("cold preview diagnostics");

        assert!(diagnostics.backend.is_some());
        assert!(diagnostics.queue_wait_ms.is_some());
        assert!(diagnostics.source_wait_ms.is_some());
        assert!(diagnostics.decode_ms.is_some());
        assert!(diagnostics.encode_ms.is_some());
        assert!(diagnostics.cache_sync_ms.is_some());
        assert!(diagnostics.cache_commit_ms.is_some());
        assert!(diagnostics.total_ms.is_some());
        assert!(
            diagnostics.total_ms.unwrap()
                >= diagnostics.decode_ms.unwrap()
                    + diagnostics.encode_ms.unwrap()
                    + diagnostics.cache_sync_ms.unwrap()
                    + diagnostics.cache_commit_ms.unwrap()
        );

        let warm = heif_preview(&path, cache.path(), 512).unwrap();
        assert!(
            warm.diagnostics.is_none(),
            "warm cache hits skip decode timing"
        );
    }

    #[test]
    fn reads_regular_image_dimensions_without_libraw() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.png");
        RgbImage::from_pixel(7, 5, Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();

        assert_eq!(
            dimensions(&path).unwrap(),
            ImageDimensions {
                width: 7,
                height: 5
            }
        );
    }

    #[test]
    fn near_full_raw_preview_accepts_orientation_and_active_area_difference() {
        assert!(covers_raw_source(
            ImageDimensions {
                width: 7_008,
                height: 4_672,
            },
            ImageDimensions {
                width: 4_688,
                height: 7_028,
            },
        ));
        assert!(!covers_raw_source(
            ImageDimensions {
                width: 1_616,
                height: 1_080,
            },
            ImageDimensions {
                width: 6_240,
                height: 4_168,
            },
        ));
    }

    #[test]
    fn raw_preview_reports_embedded_and_development_failures() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("broken.arw");
        fs::write(&path, b"not a raw image").unwrap();

        let error = libraw::preview(&path, 4_096, true).err().unwrap();
        assert!(error.contains("embedded preview failed"));
        assert!(error.contains("RAW development failed"));
    }

    #[test]
    #[ignore = "requires OXY_RAW_FIXTURE to point to a camera RAW file"]
    fn extracts_preview_from_raw_fixture() {
        let raw_path =
            fs::canonicalize(workspace_path(std::env::var_os("OXY_RAW_FIXTURE").unwrap())).unwrap();
        let directory = tempfile::tempdir().unwrap();

        let raw_size = raw_dimensions(&raw_path).unwrap();
        let preview = raw_preview(&raw_path, directory.path(), 512).unwrap();
        let preview_size = dimensions(&preview.path).unwrap();

        assert!(raw_size.width > 0 && raw_size.height > 0);
        assert!(preview_size.width <= 512 && preview_size.height <= 512);
        assert_eq!(
            raw_size.width >= raw_size.height,
            preview_size.width >= preview_size.height,
            "preview orientation must match RAW output dimensions"
        );
        assert!(preview.path.is_file());
    }

    #[test]
    #[ignore = "requires OXY_RAW_FIXTURE to point to a camera RAW file with an embedded JPEG"]
    fn preserves_embedded_jpeg_for_loupe_fixture() {
        let raw_path =
            fs::canonicalize(workspace_path(std::env::var_os("OXY_RAW_FIXTURE").unwrap())).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let embedded = match libraw::preview(&raw_path, 4_096, true).unwrap() {
            libraw::Preview::EmbeddedJpeg(data) => data,
            libraw::Preview::Image(_) => panic!("fixture did not expose an embedded JPEG"),
        };

        let preview = raw_preview(&raw_path, directory.path(), 4_096).unwrap();
        assert_eq!(preview.kind, PreviewKind::Embedded);
        assert_eq!(fs::read(&preview.path).unwrap(), embedded);

        let raw_size = raw_dimensions(&raw_path).unwrap();
        let reader = ImageReader::with_format(Cursor::new(&embedded), ImageFormat::Jpeg);
        let mut decoder = reader.into_decoder().unwrap();
        let orientation = decoder.orientation().unwrap();
        let mut oriented_preview = DynamicImage::from_decoder(decoder).unwrap();
        oriented_preview.apply_orientation(orientation);
        let preview_size = ImageDimensions {
            width: oriented_preview.width(),
            height: oriented_preview.height(),
        };
        assert_eq!(
            raw_size.width >= raw_size.height,
            preview_size.width >= preview_size.height,
            "direct embedded preview orientation must match RAW output dimensions"
        );

        assert!(
            matches!(
                libraw::preview(&raw_path, 512, true).unwrap(),
                libraw::Preview::EmbeddedJpeg(_)
            ),
            "thumbnail requests must preserve the selected embedded JPEG"
        );
    }

    #[test]
    #[ignore = "requires OXY_RAW_FIXTURE to point to a camera RAW file"]
    fn resolves_full_detail_raw_fixture() {
        let raw_path =
            fs::canonicalize(workspace_path(std::env::var_os("OXY_RAW_FIXTURE").unwrap())).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let raw_size = raw_dimensions(&raw_path).unwrap();
        let started = Instant::now();
        let full = raw_full(&raw_path, directory.path()).unwrap();

        eprintln!("{}: full RAW={:?}", raw_path.display(), started.elapsed());
        assert!(
            full.kind == PreviewKind::Developed
                || (full.kind == PreviewKind::Embedded
                    && covers_raw_source(
                        ImageDimensions {
                            width: full.width,
                            height: full.height,
                        },
                        raw_size,
                    ))
        );
        assert!(full.path.is_file());
        assert_eq!(raw_full(&raw_path, directory.path()).unwrap(), full);
    }

    #[test]
    #[ignore = "requires OXY_HEIF_FIXTURE to point to a camera HEIF file"]
    fn decodes_full_resolution_heif_fixture() {
        let heif_path = fs::canonicalize(workspace_path(
            std::env::var_os("OXY_HEIF_FIXTURE").unwrap(),
        ))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let original_size = libheif::dimensions(&heif_path).unwrap();
        let started = Instant::now();
        let full = heif_full(&heif_path, directory.path()).unwrap();

        eprintln!("{}: full HEIF={:?}", heif_path.display(), started.elapsed());
        assert_eq!(full.kind, PreviewKind::Decoded);
        assert_eq!(
            (full.width, full.height),
            (original_size.width, original_size.height)
        );
        assert!(full.path.is_file());
        assert_eq!(heif_full(&heif_path, directory.path()).unwrap(), full);

        let mut decoder = ImageReader::open(&full.path)
            .unwrap()
            .into_decoder()
            .unwrap();
        assert!(decoder.icc_profile().unwrap().is_some());
    }

    #[test]
    #[ignore = "requires OXY_HEIF_FIXTURE and macOS Quick Look"]
    fn generates_large_system_fallback_for_heif_fixture() {
        let heif_path = fs::canonicalize(workspace_path(
            std::env::var_os("OXY_HEIF_FIXTURE").unwrap(),
        ))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let original = libheif::dimensions(&heif_path).unwrap();
        let preview = system_preview(
            &heif_path,
            directory.path(),
            original.width.max(original.height).min(8_192),
        )
        .unwrap();

        assert_eq!(preview.kind, PreviewKind::System);
        assert_eq!(
            (preview.width, preview.height),
            (original.width, original.height)
        );
    }

    #[test]
    #[ignore = "requires local ARW/HIF fixtures; run explicitly in release mode"]
    fn fixture_preview_performance_budgets() {
        let fixture_dir = std::env::var_os("OXY_MEDIA_FIXTURE_DIR").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/media"),
            workspace_path,
        );
        let mut fixtures = fs::read_dir(&fixture_dir)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture_dir.display()))
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                matches!(
                    path.extension()
                        .and_then(|extension| extension.to_str())
                        .map(str::to_ascii_lowercase)
                        .as_deref(),
                    Some("arw") | Some("hif")
                )
            })
            .collect::<Vec<_>>();
        fixtures.sort();
        assert!(!fixtures.is_empty(), "no ARW/HIF fixtures found");

        for fixture in fixtures {
            let cache = tempfile::tempdir().unwrap();
            let preview = |size| {
                if fixture
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("arw"))
                {
                    raw_preview(&fixture, cache.path(), size)
                } else {
                    system_preview(&fixture, cache.path(), size)
                }
            };

            let thumbnail_started = Instant::now();
            preview(512).unwrap();
            let thumbnail_elapsed = thumbnail_started.elapsed();

            let loupe_started = Instant::now();
            let loupe_path = preview(4_096).unwrap();
            let loupe_elapsed = loupe_started.elapsed();

            let warm_started = Instant::now();
            assert_eq!(preview(4_096).unwrap(), loupe_path);
            let warm_elapsed = warm_started.elapsed();

            eprintln!(
                "{}: thumbnail={thumbnail_elapsed:?} loupe={loupe_elapsed:?} warm={warm_elapsed:?}",
                fixture.display()
            );
            assert!(
                thumbnail_elapsed < Duration::from_millis(800),
                "{} thumbnail took {thumbnail_elapsed:?}",
                fixture.display()
            );
            assert!(
                loupe_elapsed < Duration::from_millis(800),
                "{} loupe took {loupe_elapsed:?}",
                fixture.display()
            );
            assert!(
                warm_elapsed < Duration::from_millis(150),
                "{} warm cache took {warm_elapsed:?}",
                fixture.display()
            );
        }
    }

    #[test]
    #[ignore = "requires OXY_HEIF_FIXTURE to point to a HEIF file"]
    fn heif_decode_performance_budget() {
        let heif_path = fs::canonicalize(workspace_path(
            std::env::var_os("OXY_HEIF_FIXTURE").unwrap(),
        ))
        .unwrap();

        // Cold full decode + PNG cache write.
        let cache_full = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let full = heif_full(&heif_path, cache_full.path()).unwrap();
        let full_elapsed = started.elapsed();
        eprintln!("HEIF full decode: {full_elapsed:?}");
        assert!(
            full_elapsed < Duration::from_secs(3),
            "full decode took {full_elapsed:?}"
        );

        // Warm full-detail cache hit.
        let started = Instant::now();
        assert_eq!(
            heif_full(&heif_path, cache_full.path()).unwrap().path,
            full.path
        );
        let warm_full = started.elapsed();
        eprintln!("HEIF warm full: {warm_full:?}");
        assert!(
            warm_full < Duration::from_millis(100),
            "warm full took {warm_full:?}"
        );

        // Cold 512 px thumbnail via decode_scaled (8-bit fast path).
        let cache_thumb = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let thumb = heif_preview(&heif_path, cache_thumb.path(), 512).unwrap();
        let thumb_elapsed = started.elapsed();
        eprintln!("HEIF thumbnail (512): {thumb_elapsed:?}");
        assert!(
            thumb_elapsed < Duration::from_millis(800),
            "thumbnail took {thumb_elapsed:?}"
        );
        assert!(thumb.width <= 512 && thumb.height <= 512);

        // Cold 4096 px loupe preview via decode_scaled.
        let cache_loupe = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let loupe = heif_preview(&heif_path, cache_loupe.path(), 4_096).unwrap();
        let loupe_elapsed = started.elapsed();
        eprintln!("HEIF loupe (4096): {loupe_elapsed:?}");
        assert!(
            loupe_elapsed < Duration::from_millis(1_500),
            "loupe took {loupe_elapsed:?}"
        );
        assert!(loupe.width <= 4_096 && loupe.height <= 4_096);

        // Warm loupe cache hit.
        let started = Instant::now();
        assert_eq!(
            heif_preview(&heif_path, cache_loupe.path(), 4_096)
                .unwrap()
                .path,
            loupe.path
        );
        let warm_loupe = started.elapsed();
        eprintln!("HEIF warm loupe: {warm_loupe:?}");
        assert!(
            warm_loupe < Duration::from_millis(100),
            "warm loupe took {warm_loupe:?}"
        );
    }
}
