mod libraw;

use image::{DynamicImage, ImageReader, codecs::jpeg::JpegEncoder};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{BufReader, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use tempfile::NamedTempFile;
use thiserror::Error;

const LIBRAW_CACHE_VERSION: &str = "libraw-0.22.1-v4";
const SYSTEM_CACHE_VERSION: &str = "system-preview-v2";
const LOUPE_PREVIEW_THRESHOLD: u32 = 2_048;
static RAW_THUMBNAIL_DECODE_LOCK: Mutex<()> = Mutex::new(());
static RAW_LOUPE_DECODE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("format requires a native decoder that is not available")]
    NativeDecoderUnavailable,
    #[error("LibRaw failed for {path}: {message}")]
    LibRaw { path: PathBuf, message: String },
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

    raster().or_else(|_| raw_dimensions(path))
}

pub fn raw_dimensions(path: &Path) -> Result<ImageDimensions, MediaError> {
    libraw::dimensions(path).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    })
}

pub fn raw_preview(path: &Path, cache_dir: &Path, max_size: u32) -> Result<PathBuf, MediaError> {
    let max_size = max_size.max(1);
    fs::create_dir_all(cache_dir)?;
    let destination = cache_dir.join(format!(
        "{}.jpg",
        preview_cache_key(path, LIBRAW_CACHE_VERSION, max_size)?
    ));
    if destination.is_file() {
        return Ok(destination);
    }

    let decode_lock = if max_size >= LOUPE_PREVIEW_THRESHOLD {
        &RAW_LOUPE_DECODE_LOCK
    } else {
        &RAW_THUMBNAIL_DECODE_LOCK
    };
    let _decode_guard = decode_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if destination.is_file() {
        return Ok(destination);
    }

    let preserve_embedded_jpeg = max_size >= LOUPE_PREVIEW_THRESHOLD;
    let preview = libraw::preview(path, max_size, preserve_embedded_jpeg).map_err(|message| {
        MediaError::LibRaw {
            path: path.to_owned(),
            message,
        }
    })?;
    match preview {
        libraw::Preview::EmbeddedJpeg(data) => write_bytes_atomically(&data, &destination)?,
        libraw::Preview::Image(image) => write_jpeg_atomically(&image, &destination)?,
    }
    Ok(destination)
}

pub fn system_preview(path: &Path, cache_dir: &Path, max_size: u32) -> Result<PathBuf, MediaError> {
    fs::create_dir_all(cache_dir)?;
    let destination = cache_dir.join(format!(
        "{}.png",
        preview_cache_key(path, SYSTEM_CACHE_VERSION, max_size)?
    ));
    if destination.is_file() {
        return Ok(destination);
    }
    generate_system_preview(path, &destination, max_size)?;
    Ok(destination)
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

fn write_jpeg_atomically(image: &DynamicImage, destination: &Path) -> Result<(), MediaError> {
    let mut temporary =
        NamedTempFile::new_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    JpegEncoder::new_with_quality(&mut temporary, 90).encode_image(image)?;
    persist_atomically(temporary, destination)
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
    use image::{Rgb, RgbImage};
    use std::time::{Duration, Instant};

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
        let preview_path = raw_preview(&raw_path, directory.path(), 512).unwrap();
        let preview_size = dimensions(&preview_path).unwrap();

        assert!(raw_size.width > 0 && raw_size.height > 0);
        assert!(preview_size.width <= 512 && preview_size.height <= 512);
        assert_eq!(
            raw_size.width >= raw_size.height,
            preview_size.width >= preview_size.height,
            "preview orientation must match RAW output dimensions"
        );
        assert!(preview_path.is_file());
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

        let preview_path = raw_preview(&raw_path, directory.path(), 4_096).unwrap();
        assert_eq!(fs::read(&preview_path).unwrap(), embedded);

        let raw_size = raw_dimensions(&raw_path).unwrap();
        let preview_size = dimensions(&preview_path).unwrap();
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
}
