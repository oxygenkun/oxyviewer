#[cfg(target_os = "macos")]
mod apple_image_io;
mod ffmpeg_heif;
mod heif;
mod heif_service;
mod libraw;
#[cfg(target_os = "windows")]
mod windows_wic;

pub use heif_service::{DEFAULT_TILE_SIZE, HeifBackend, HeifDecodeService, HeifTile, TileSink};
use image::{DynamicImage, ImageEncoder, ImageReader, codecs::jpeg::JpegEncoder};
use oxy_domain::{PreviewDiagnostics, PreviewKind, PreviewResult};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{BufReader, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, LazyLock, Mutex},
    time::Instant,
};
use tempfile::NamedTempFile;
use thiserror::Error;

const LIBRAW_CACHE_VERSION: &str = "libraw-0.22.1-v5";
const LIBRAW_FULL_CACHE_VERSION: &str = "libraw-0.22.1-full-detail-v2";
// Bumped from `libheif-1.23-sdr-v1` (16-bit PNG) to an 8-bit sRGB JPEG with an
// embedded ICC profile, unifying the cache format across every preview stage
// and format. Old PNG caches are rebuildable and simply ignored.
const HEIF_FULL_CACHE_VERSION: &str = "heif-sdr-jpeg-v1";
const HEIF_CACHE_VERSION: &str = "heif-native-preview-v5";
const SYSTEM_CACHE_VERSION: &str = "system-preview-v2";
const LOUPE_PREVIEW_THRESHOLD: u32 = 2_048;
/// Cache sizes shared by every format's progressive pipeline. A request for a
/// smaller size may be satisfied by any larger cached entry (see
/// [`larger_cached_preview`]).
const PREVIEW_CACHE_SIZES: [u32; 2] = [512, 4_096];
// Full-resolution RAW development (potentially tens of seconds) stays on its
// own lane so it never blocks the unified thumbnail/loupe gate. The gate below
// covers the progressive stages (512 / 4096) for every format.
static RAW_FULL_DECODE_LOCK: Mutex<()> = Mutex::new(());
static DECODE_GATE: DecodeGate = DecodeGate::new();

// Per-file locks coalesce duplicate cache work after the global decode gate has
// selected the next source. Shared by every format.
static DECODE_LOCKS: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Priority for the unified decode gate. Higher priorities jump ahead of
/// lower-priority waiters but never preempt a running decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodePriority {
    /// Overscan/off-screen thumbnails — served only when nothing else wants the gate.
    Background,
    /// On-screen thumbnails and filmstrip — served before background work.
    Visible,
    /// The currently-selected loupe image — served first.
    Foreground,
}

/// Backwards-compatible alias. HEIF code historically used `HeifDecodePriority`;
/// the gate is now format-agnostic but the name is retained to avoid churning
/// `heif_service.rs` and the public re-exports.
pub type HeifDecodePriority = DecodePriority;

struct DecodeGate {
    state: Mutex<DecodeGateState>,
    ready: Condvar,
}

#[derive(Default)]
struct DecodeGateState {
    active: bool,
    foreground_waiters: usize,
    visible_waiters: usize,
}

struct DecodePermit<'a> {
    gate: &'a DecodeGate,
}

impl DecodeGate {
    const fn new() -> Self {
        Self {
            state: Mutex::new(DecodeGateState {
                active: false,
                foreground_waiters: 0,
                visible_waiters: 0,
            }),
            ready: Condvar::new(),
        }
    }

    fn acquire(&self, priority: DecodePriority) -> DecodePermit<'_> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if priority == DecodePriority::Foreground {
            state.foreground_waiters += 1;
        } else if priority == DecodePriority::Visible {
            state.visible_waiters += 1;
        }
        while state.active
            || match priority {
                DecodePriority::Foreground => false,
                DecodePriority::Visible => state.foreground_waiters > 0,
                DecodePriority::Background => {
                    state.foreground_waiters > 0 || state.visible_waiters > 0
                }
            }
        {
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        if priority == DecodePriority::Foreground {
            state.foreground_waiters -= 1;
        } else if priority == DecodePriority::Visible {
            state.visible_waiters -= 1;
        }
        state.active = true;
        DecodePermit { gate: self }
    }
}

impl Drop for DecodePermit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.active = false;
        self.gate.ready.notify_all();
    }
}

/// Acquire the unified decode gate. Higher-priority waiters are served before
/// lower-priority ones; a running decode is never preempted (caller opted into
/// the "order pending only" scheduling policy).
pub(crate) fn acquire_decode(priority: DecodePriority) -> impl Drop {
    DECODE_GATE.acquire(priority)
}

/// Backwards-compatible alias for [`acquire_decode`].
pub(crate) fn acquire_heif_decode(priority: DecodePriority) -> impl Drop {
    acquire_decode(priority)
}

/// Look up or create a per-file `Mutex`, clone the `Arc`, then lock it.
/// Returns `(Arc<Mutex<()>>, MutexGuard)` — caller must keep the `Arc` alive
/// alongside the guard (it is dropped last due to reverse-order drop).
fn acquire_file_lock(cache_key: &str) -> (Arc<Mutex<()>>, std::sync::MutexGuard<'static, ()>) {
    let arc: Arc<Mutex<()>> = DECODE_LOCKS
        .lock()
        .unwrap()
        .entry(cache_key.to_owned())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    // Access the Mutex via raw pointer to decouple the guard's lifetime from
    // the local `arc` binding. This lets us return both the Arc and the guard.
    // Safety: the Mutex lives inside the static DECODE_LOCKS HashMap behind an
    // Arc that is never removed; the returned Arc keeps it alive.
    let mutex: &'static Mutex<()> = unsafe { &*Arc::as_ptr(&arc) };
    let guard = mutex.lock().unwrap_or_else(|e| e.into_inner());
    (arc, guard)
}

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
        .or_else(|_| heif::dimensions(path))
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

    let preserve_embedded_jpeg = max_size >= LOUPE_PREVIEW_THRESHOLD;
    let preview = libraw::preview(path, max_size, preserve_embedded_jpeg).map_err(|message| {
        MediaError::LibRaw {
            path: path.to_owned(),
            message,
        }
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

    let _decode_guard = RAW_FULL_DECODE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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

    let image = heif::decode_primary(path)?;
    let temporary = NamedTempFile::new_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    heif::write_srgb_jpeg(&image, temporary.path(), 95)?;
    persist_atomically(temporary, &destination)?;
    preview_result(destination, PreviewKind::Decoded)
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
    #[cfg(target_os = "macos")]
    let mut fallback_reason = None;
    #[cfg(target_os = "macos")]
    if crate::apple_image_io::can_decode(path).is_ok() {
        match crate::apple_image_io::decode_rgba8(path, max_size) {
            Ok(image) => return Ok((image, "Apple ImageIO thumbnail", None)),
            Err(error) => fallback_reason = Some(error.to_string()),
        }
    }
    #[cfg(target_os = "windows")]
    let mut fallback_reason = None;
    #[cfg(target_os = "windows")]
    if crate::ffmpeg_heif::can_decode(path).is_ok() {
        match crate::ffmpeg_heif::decode_scaled_preview(path, max_size) {
            Ok(image) => return Ok((image, "FFmpeg auxiliary preview", None)),
            Err(error) => fallback_reason = Some(error.to_string()),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let fallback_reason = None;
    let image = heif::decode_scaled(path, max_size)?;
    #[cfg(target_os = "macos")]
    return Ok((image, "libheif scaled preview", fallback_reason));
    #[cfg(not(target_os = "macos"))]
    Ok((image, "libheif scaled preview", fallback_reason))
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
        PreviewPriority::Nearby => DecodePriority::Background,
        PreviewPriority::Visible => DecodePriority::Visible,
        PreviewPriority::Loupe => DecodePriority::Foreground,
    }
}

/// Whether a given asset kind needs server-side decoding. Raster formats that
/// the web view can render directly (JPEG/PNG/WebP) are served as originals;
/// everything else flows through the unified pipeline.
pub fn needs_decode(kind: oxy_domain::AssetKind) -> bool {
    use oxy_domain::AssetKind;
    matches!(kind, AssetKind::Raw | AssetKind::Heif | AssetKind::Tiff)
}

/// Unified preview dispatcher. Subsumes the per-format `match kind` branch that
/// used to live in the Tauri `get_preview` command. Every format that needs
/// decoding funnels through this entry point, so the cache/gate/fallback policy
/// is defined in exactly one place and new formats only need to extend this
/// function.
///
/// Stage dispatch:
/// - `FullDetail` for RAW → full-resolution development ([`raw_full`]).
/// - `FullDetail` for HEIF → full-resolution JPEG ([`heif_full`]); the Tile
///   session is started separately by the frontend for streaming display.
/// - `Thumbnail` / `LoupePreview` for RAW/HEIF → progressive preview with the
///   supplied priority.
/// - TIFF and any future raster-needing format → [`system_preview`].
pub fn preview(
    path: &Path,
    cache_dir: &Path,
    mode: oxy_domain::PreviewMode,
    max_size: u32,
    priority: DecodePriority,
    kind: oxy_domain::AssetKind,
) -> Result<PreviewResult, MediaError> {
    use oxy_domain::{AssetKind, PreviewMode};
    match (kind, mode) {
        (AssetKind::Raw, PreviewMode::FullDetail) => raw_full(path, cache_dir),
        (AssetKind::Heif, PreviewMode::FullDetail) => heif_full(path, cache_dir).or_else(|error| {
            // Keep the documented fallback: if the full-detail decode fails,
            // serve the largest progressive preview at foreground priority so
            // the loupe is never left empty.
            eprintln!(
                "full-detail HEIF decode failed for {}: {error}",
                path.display()
            );
            heif_preview_with_priority(path, cache_dir, 8_192, DecodePriority::Foreground)
        }),
        (AssetKind::Raw, _) => {
            raw_preview_with_priority(path, cache_dir, max_size, priority).or_else(|error| {
                // Fallback to the OS generator, preserving the original error
                // in the message so diagnostics stay actionable.
                match system_preview(path, cache_dir, max_size) {
                    Ok(result) => Ok(result),
                    Err(system_error) => Err(MediaError::LibRaw {
                        path: path.to_owned(),
                        message: format!("{error}; fallback failed: {system_error}"),
                    }),
                }
            })
        }
        (AssetKind::Heif, _) => heif_preview_with_priority(path, cache_dir, max_size, priority),
        (AssetKind::Tiff, _) => system_preview(path, cache_dir, max_size),
        // Future formats: route through the system generator until a native
        // adapter is registered. Falling through here also keeps the exhaustiveness
        // check honest when new AssetKind variants are added.
        (_, _) => system_preview(path, cache_dir, max_size),
    }
}

fn preview_cache_key(path: &Path, backend: &str, max_size: u32) -> Result<String, MediaError> {
    // Key previews by physical file identity rather than by the library root
    // used to reach it. Parent/child roots (and symlink aliases) therefore
    // converge on one cache entry for the same source image.
    let canonical_path = path.canonicalize()?;
    let metadata = fs::metadata(&canonical_path)?;
    let mut hasher = DefaultHasher::new();
    canonical_path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata.modified().ok().hash(&mut hasher);
    backend.hash(&mut hasher);
    max_size.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

fn write_jpeg_atomically(
    image: &DynamicImage,
    destination: &Path,
    quality: u8,
) -> Result<(), MediaError> {
    write_jpeg_atomically_with_icc(image, destination, quality, None)
}

#[derive(Debug, Clone, Copy)]
struct CacheWriteTiming {
    encode_ms: u64,
    sync_ms: u64,
    commit_ms: u64,
}

fn write_jpeg_atomically_timed(
    image: &DynamicImage,
    destination: &Path,
    quality: u8,
) -> Result<CacheWriteTiming, MediaError> {
    let temporary = tempfile::Builder::new()
        .suffix(".jpg")
        .tempfile_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    let encode_started = Instant::now();
    #[cfg(target_os = "macos")]
    apple_image_io::write_jpeg(image, temporary.path(), quality)?;
    #[cfg(not(target_os = "macos"))]
    {
        let mut output = temporary.as_file();
        JpegEncoder::new_with_quality(&mut output, quality).encode_image(image)?;
    }
    let encode_ms = duration_ms(encode_started);
    let sync_started = Instant::now();
    temporary.as_file().sync_all()?;
    let sync_ms = duration_ms(sync_started);
    let commit_started = Instant::now();
    persist_noclobber(temporary, destination)?;
    Ok(CacheWriteTiming {
        encode_ms,
        sync_ms,
        commit_ms: duration_ms(commit_started),
    })
}

/// Encode `image` as JPEG, optionally embedding an ICC profile in an APP2 chunk.
/// Used by the unified cache layer so every preview stage and format lands as a
/// color-managed JPEG on disk.
fn write_jpeg_atomically_with_icc(
    image: &DynamicImage,
    destination: &Path,
    quality: u8,
    icc: Option<Vec<u8>>,
) -> Result<(), MediaError> {
    let mut temporary =
        NamedTempFile::new_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    let mut encoder = JpegEncoder::new_with_quality(&mut temporary, quality);
    if let Some(profile) = icc {
        encoder
            .set_icc_profile(profile)
            .map_err(|error| MediaError::Color(error.to_string()))?;
    }
    encoder.encode_image(image)?;
    persist_atomically(temporary, destination)
}

fn preview_result(path: PathBuf, kind: PreviewKind) -> Result<PreviewResult, MediaError> {
    let size = dimensions(&path)?;
    Ok(PreviewResult {
        path,
        width: size.width,
        height: size.height,
        kind,
        stage: None,
        diagnostics: None,
    })
}

fn write_bytes_atomically(data: &[u8], destination: &Path) -> Result<(), MediaError> {
    let mut temporary =
        NamedTempFile::new_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    temporary.write_all(data)?;
    persist_atomically(temporary, destination)
}

fn persist_atomically(temporary: NamedTempFile, destination: &Path) -> Result<(), MediaError> {
    temporary.as_file().sync_all()?;

    persist_noclobber(temporary, destination)
}

fn persist_noclobber(temporary: NamedTempFile, destination: &Path) -> Result<(), MediaError> {
    match temporary.persist_noclobber(destination) {
        Ok(_) => Ok(()),
        Err(_error) if destination.is_file() => Ok(()),
        Err(error) => Err(MediaError::Io(error.error)),
    }
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

#[cfg(not(target_os = "macos"))]
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
        sync::mpsc,
        thread,
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
    fn decode_gate_prefers_foreground_then_visible_then_background() {
        // The gate is now format-agnostic; the priority contract (loupe first,
        // visible second, overscan last) is preserved unchanged.
        let gate = Arc::new(DecodeGate::new());
        let active = gate.acquire(DecodePriority::Background);
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let spawn_waiter = |priority| {
            let gate = gate.clone();
            let acquired_tx = acquired_tx.clone();
            thread::spawn(move || {
                let _permit = gate.acquire(priority);
                acquired_tx.send(priority).unwrap();
            })
        };
        let nearby = spawn_waiter(DecodePriority::Background);
        let visible = spawn_waiter(DecodePriority::Visible);
        let loupe = spawn_waiter(DecodePriority::Foreground);

        let started = Instant::now();
        loop {
            let state = gate.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.foreground_waiters == 1 && state.visible_waiters == 1 {
                break;
            }
            drop(state);
            assert!(started.elapsed() < Duration::from_secs(1));
            thread::yield_now();
        }
        drop(active);

        assert_eq!(
            acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            DecodePriority::Foreground
        );
        assert_eq!(
            acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            DecodePriority::Visible
        );
        assert_eq!(
            acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            DecodePriority::Background
        );
        nearby.join().unwrap();
        visible.join().unwrap();
        loupe.join().unwrap();
    }

    #[test]
    fn decode_gate_orders_visible_thumbnail_before_nearby() {
        // Regression for the unified gate: a visible thumbnail that arrives
        // *after* a nearby one must still be served first. This is the core
        // "scroll into view jumps the queue" guarantee for every format.
        let gate = Arc::new(DecodeGate::new());
        let active = gate.acquire(DecodePriority::Foreground);
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let spawn_waiter = |priority| {
            let gate = gate.clone();
            let acquired_tx = acquired_tx.clone();
            thread::spawn(move || {
                let _permit = gate.acquire(priority);
                acquired_tx.send(priority).unwrap();
            })
        };
        // Nearby arrives first, visible second.
        let nearby = spawn_waiter(DecodePriority::Background);
        let visible = spawn_waiter(DecodePriority::Visible);

        let started = Instant::now();
        loop {
            let state = gate.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.visible_waiters == 1 {
                break;
            }
            drop(state);
            assert!(started.elapsed() < Duration::from_secs(1));
            thread::yield_now();
        }
        drop(active);

        assert_eq!(
            acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            DecodePriority::Visible
        );
        assert_eq!(
            acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            DecodePriority::Background
        );
        visible.join().unwrap();
        nearby.join().unwrap();
    }

    #[test]
    fn cache_key_changes_with_backend_and_size() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.jpg");
        RgbImage::from_pixel(4, 3, Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();

        let first = preview_cache_key(&path, "one", 512).unwrap();
        let different_backend = preview_cache_key(&path, "two", 512).unwrap();
        let different_size = preview_cache_key(&path, "one", 2_048).unwrap();

        assert_ne!(first, different_backend);
        assert_ne!(first, different_size);
    }

    #[cfg(unix)]
    #[test]
    fn cache_key_is_shared_across_paths_to_the_same_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.jpg");
        let alias = directory.path().join("image-alias.jpg");
        RgbImage::from_pixel(4, 3, Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        std::os::unix::fs::symlink(&path, &alias).unwrap();

        assert_eq!(
            preview_cache_key(&path, "jpeg", 512).unwrap(),
            preview_cache_key(&alias, "jpeg", 512).unwrap()
        );
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
        let raw_tag = "libraw-0.22.1-v5";
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
                libraw::preview(&raw_path, 512, false).unwrap(),
                libraw::Preview::Image(_)
            ),
            "thumbnail requests must keep the resize and re-encode path"
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
        let original_size = heif::dimensions(&heif_path).unwrap();
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
        let original = heif::dimensions(&heif_path).unwrap();
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
        let fixture_dir = std::env::var_os("OXY_MEDIA_FIXTURE_DIR")
            .map(workspace_path)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/media")
            });
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
        eprintln!("HEIF full decode: {:?}", full_elapsed);
        assert!(
            full_elapsed < Duration::from_secs(3),
            "full decode took {:?}",
            full_elapsed
        );

        // Warm full-detail cache hit.
        let started = Instant::now();
        assert_eq!(
            heif_full(&heif_path, cache_full.path()).unwrap().path,
            full.path
        );
        let warm_full = started.elapsed();
        eprintln!("HEIF warm full: {:?}", warm_full);
        assert!(
            warm_full < Duration::from_millis(100),
            "warm full took {:?}",
            warm_full
        );

        // Cold 512 px thumbnail via decode_scaled (8-bit fast path).
        let cache_thumb = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let thumb = heif_preview(&heif_path, cache_thumb.path(), 512).unwrap();
        let thumb_elapsed = started.elapsed();
        eprintln!("HEIF thumbnail (512): {:?}", thumb_elapsed);
        assert!(
            thumb_elapsed < Duration::from_millis(800),
            "thumbnail took {:?}",
            thumb_elapsed
        );
        assert!(thumb.width <= 512 && thumb.height <= 512);

        // Cold 4096 px loupe preview via decode_scaled.
        let cache_loupe = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let loupe = heif_preview(&heif_path, cache_loupe.path(), 4_096).unwrap();
        let loupe_elapsed = started.elapsed();
        eprintln!("HEIF loupe (4096): {:?}", loupe_elapsed);
        assert!(
            loupe_elapsed < Duration::from_millis(1_500),
            "loupe took {:?}",
            loupe_elapsed
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
        eprintln!("HEIF warm loupe: {:?}", warm_loupe);
        assert!(
            warm_loupe < Duration::from_millis(100),
            "warm loupe took {:?}",
            warm_loupe
        );
    }
}
