mod ffmpeg_heif;
mod heif;
mod heif_service;
mod libraw;
#[cfg(target_os = "windows")]
mod windows_wic;

pub use heif_service::{DEFAULT_TILE_SIZE, HeifBackend, HeifDecodeService, HeifTile, TileSink};
use image::{DynamicImage, ImageReader, codecs::jpeg::JpegEncoder};
use oxy_domain::{PreviewKind, PreviewResult};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{BufReader, Write},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};
use tempfile::NamedTempFile;
use thiserror::Error;

const LIBRAW_CACHE_VERSION: &str = "libraw-0.22.1-v5";
const LIBRAW_FULL_CACHE_VERSION: &str = "libraw-0.22.1-full-detail-v2";
const HEIF_FULL_CACHE_VERSION: &str = "libheif-1.23-sdr-v1";
const HEIF_CACHE_VERSION: &str = "libheif-1.23-preview-v4";
const SYSTEM_CACHE_VERSION: &str = "system-preview-v2";
const LOUPE_PREVIEW_THRESHOLD: u32 = 2_048;
static RAW_THUMBNAIL_DECODE_LOCK: Mutex<()> = Mutex::new(());
static RAW_LOUPE_DECODE_LOCK: Mutex<()> = Mutex::new(());
static RAW_FULL_DECODE_LOCK: Mutex<()> = Mutex::new(());

// Per-file HEIF decode locks: different files decode concurrently, same file
// is serialised to avoid redundant work.
static HEIF_LOCKS: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Look up or create a per-file `Mutex`, clone the `Arc`, then lock it.
/// Returns `(Arc<Mutex<()>>, MutexGuard)` — caller must keep the `Arc` alive
/// alongside the guard (it is dropped last due to reverse-order drop).
fn acquire_heif_lock(cache_key: &str) -> (Arc<Mutex<()>>, std::sync::MutexGuard<'static, ()>) {
    let arc: Arc<Mutex<()>> = HEIF_LOCKS
        .lock()
        .unwrap()
        .entry(cache_key.to_owned())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    // Access the Mutex via raw pointer to decouple the guard's lifetime from
    // the local `arc` binding. This lets us return both the Arc and the guard.
    // Safety: the Mutex lives inside the static HEIF_LOCKS HashMap behind an
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

    let decode_lock = if max_size >= LOUPE_PREVIEW_THRESHOLD {
        &RAW_LOUPE_DECODE_LOCK
    } else {
        &RAW_THUMBNAIL_DECODE_LOCK
    };
    let _decode_guard = decode_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (suffix, kind) in [
        ("embedded.jpg", PreviewKind::Embedded),
        ("developed.jpg", PreviewKind::Developed),
    ] {
        let destination = cache_dir.join(format!("{cache_key}.{suffix}"));
        if destination.is_file() {
            return preview_result(destination, kind);
        }
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

pub fn heif_full(path: &Path, cache_dir: &Path) -> Result<PreviewResult, MediaError> {
    fs::create_dir_all(cache_dir)?;
    let cache_key = preview_cache_key(path, HEIF_FULL_CACHE_VERSION, 0)?;
    let destination = cache_dir.join(format!("{cache_key}.png"));
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }

    let source_lock_key = preview_cache_key(path, "heif-source-decode", 0)?;
    let (_heif_lock_arc, _decode_guard) = acquire_heif_lock(&source_lock_key);
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }

    let image = heif::decode_primary(path)?;
    let temporary = NamedTempFile::new_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    heif::write_srgb_png(&image, temporary.path())?;
    persist_atomically(temporary, &destination)?;
    preview_result(destination, PreviewKind::Decoded)
}

pub fn heif_preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
) -> Result<PreviewResult, MediaError> {
    let max_size = max_size.max(1);
    fs::create_dir_all(cache_dir)?;
    let cache_key = preview_cache_key(path, HEIF_CACHE_VERSION, max_size)?;
    let destination = cache_dir.join(format!("{cache_key}.jpg"));
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }
    if let Some(result) = larger_heif_preview(path, cache_dir, max_size)? {
        return Ok(result);
    }

    let source_lock_key = preview_cache_key(path, "heif-source-decode", 0)?;
    let (_heif_lock_arc, _decode_guard) = acquire_heif_lock(&source_lock_key);
    if destination.is_file() {
        return preview_result(destination, PreviewKind::Decoded);
    }
    if let Some(result) = larger_heif_preview(path, cache_dir, max_size)? {
        return Ok(result);
    }

    // Reuse full-detail cache if available to avoid redundant decoding.
    let full_cache_key = preview_cache_key(path, HEIF_FULL_CACHE_VERSION, 0)?;
    let full_cache_path = cache_dir.join(format!("{full_cache_key}.png"));
    let image = if full_cache_path.is_file() {
        image::ImageReader::open(&full_cache_path)?
            .decode()?
            .thumbnail(max_size, max_size)
    } else {
        heif::decode_scaled(path, max_size)?
    };
    write_jpeg_atomically(&image, &destination, 90)?;
    preview_result(destination, PreviewKind::Decoded)
}

fn larger_heif_preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
) -> Result<Option<PreviewResult>, MediaError> {
    for candidate_size in [512, 4_096, 8_192] {
        if candidate_size <= max_size {
            continue;
        }
        let key = preview_cache_key(path, HEIF_CACHE_VERSION, candidate_size)?;
        let candidate = cache_dir.join(format!("{key}.jpg"));
        if candidate.is_file() {
            return preview_result(candidate, PreviewKind::Decoded).map(Some);
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

fn preview_cache_key(path: &Path, backend: &str, max_size: u32) -> Result<String, MediaError> {
    let metadata = fs::metadata(path)?;
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
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
    let mut temporary =
        NamedTempFile::new_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    JpegEncoder::new_with_quality(&mut temporary, quality).encode_image(image)?;
    persist_atomically(temporary, destination)
}

fn preview_result(path: PathBuf, kind: PreviewKind) -> Result<PreviewResult, MediaError> {
    let size = dimensions(&path)?;
    Ok(PreviewResult {
        path,
        width: size.width,
        height: size.height,
        kind,
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

    #[test]
    fn larger_cached_heif_preview_satisfies_smaller_request() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.heic");
        fs::write(&path, b"heif").unwrap();
        let key = preview_cache_key(&path, HEIF_CACHE_VERSION, 4_096).unwrap();
        let cached = directory.path().join(format!("{key}.jpg"));
        write_jpeg_atomically(&DynamicImage::new_rgb8(32, 16), &cached, 90).unwrap();

        let result = larger_heif_preview(&path, directory.path(), 512)
            .unwrap()
            .unwrap();

        assert_eq!(result.path, cached);
        assert_eq!((result.width, result.height), (32, 16));
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
    fn develops_full_resolution_raw_fixture() {
        let raw_path =
            fs::canonicalize(workspace_path(std::env::var_os("OXY_RAW_FIXTURE").unwrap())).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let raw_size = raw_dimensions(&raw_path).unwrap();
        let started = Instant::now();
        let full = raw_full(&raw_path, directory.path()).unwrap();

        eprintln!("{}: full RAW={:?}", raw_path.display(), started.elapsed());
        assert_eq!(full.kind, PreviewKind::Developed);
        assert_eq!((full.width, full.height), (raw_size.width, raw_size.height));
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
