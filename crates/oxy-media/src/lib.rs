mod libraw;

use image::{DynamicImage, ImageReader, codecs::jpeg::JpegEncoder};
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File},
    hash::{Hash, Hasher},
    io::BufReader,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};
use tempfile::NamedTempFile;
use thiserror::Error;

const LIBRAW_CACHE_VERSION: &str = "libraw-0.22.1-v3";
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

    let preview = libraw::preview(path, max_size).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    })?;
    write_jpeg_atomically(&preview, &destination)?;
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
    #[ignore = "requires OXY_RAW_FIXTURE to point to a camera RAW file"]
    fn extracts_preview_from_raw_fixture() {
        let raw_path = PathBuf::from(std::env::var_os("OXY_RAW_FIXTURE").unwrap());
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
}
