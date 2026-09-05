use crate::MediaError;
use image::{DynamicImage, ImageEncoder, codecs::jpeg::JpegEncoder};
use std::{io::Write, path::Path, time::Instant};
use tempfile::NamedTempFile;

pub(crate) fn write_jpeg_atomically(
    image: &DynamicImage,
    destination: &Path,
    quality: u8,
) -> Result<(), MediaError> {
    write_jpeg_atomically_with_icc(image, destination, quality, None)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CacheWriteTiming {
    pub(crate) encode_ms: u64,
    pub(crate) sync_ms: u64,
    pub(crate) commit_ms: u64,
}

pub(crate) fn write_jpeg_atomically_timed(
    image: &DynamicImage,
    destination: &Path,
    quality: u8,
) -> Result<CacheWriteTiming, MediaError> {
    let temporary = tempfile::Builder::new()
        .suffix(".jpg")
        .tempfile_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    let encode_started = Instant::now();
    #[cfg(target_os = "macos")]
    crate::apple_image_io::write_jpeg(image, temporary.path(), quality)?;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
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

pub(crate) fn write_bytes_atomically(data: &[u8], destination: &Path) -> Result<(), MediaError> {
    let mut temporary =
        NamedTempFile::new_in(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    temporary.write_all(data)?;
    persist_atomically(temporary, destination)
}

pub(crate) fn persist_atomically(
    temporary: NamedTempFile,
    destination: &Path,
) -> Result<(), MediaError> {
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

fn duration_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageDecoder, ImageReader};
    use std::fs;

    #[test]
    fn byte_writes_preserve_existing_artifact_and_remove_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("preview.jpg");
        write_bytes_atomically(b"first", &destination).unwrap();
        write_bytes_atomically(b"second", &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"first");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_commit_keeps_destination_directory_and_cleans_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("preview.jpg");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep"), b"keep").unwrap();

        assert!(matches!(
            write_bytes_atomically(b"image", &destination),
            Err(MediaError::Io(_))
        ));
        assert_eq!(fs::read(destination.join("keep")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn missing_parent_is_not_created_by_writer() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("missing");
        assert!(write_bytes_atomically(b"image", &parent.join("preview.jpg")).is_err());
        assert!(!parent.exists());
    }

    #[test]
    fn jpeg_writers_produce_decodable_images_without_overwriting() {
        let directory = tempfile::tempdir().unwrap();
        for timed in [false, true] {
            let destination = directory.path().join(format!("{timed}.jpg"));
            let image = DynamicImage::new_rgb8(32, 16);
            if timed {
                write_jpeg_atomically_timed(&image, &destination, 90).unwrap();
            } else {
                write_jpeg_atomically(&image, &destination, 90).unwrap();
            }
            assert_eq!(image::image_dimensions(&destination).unwrap(), (32, 16));
            ImageReader::open(&destination).unwrap().decode().unwrap();
            let original = fs::read(&destination).unwrap();
            let replacement = DynamicImage::new_rgb8(8, 8);
            if timed {
                write_jpeg_atomically_timed(&replacement, &destination, 90).unwrap();
            } else {
                write_jpeg_atomically(&replacement, &destination, 90).unwrap();
            }
            assert_eq!(fs::read(&destination).unwrap(), original);
        }
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn jpeg_writer_preserves_optional_icc_profile() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("profile.jpg");
        let profile = lcms2::Profile::new_srgb().icc().unwrap();
        write_jpeg_atomically_with_icc(
            &DynamicImage::new_rgb8(32, 16),
            &destination,
            90,
            Some(profile.clone()),
        )
        .unwrap();
        let mut decoder = ImageReader::open(&destination)
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.icc_profile().unwrap(), Some(profile));
    }
}
