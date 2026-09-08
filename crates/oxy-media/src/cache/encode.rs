use crate::{
    MediaError,
    presentation::{ArtifactContract, ColorState},
};
use image::{DynamicImage, ImageEncoder, codecs::jpeg::JpegEncoder};
#[cfg(target_os = "macos")]
use std::fs;
use std::{io::Write, path::Path, time::Instant};
use tempfile::NamedTempFile;

pub(crate) fn cache_tempfile(
    destination: &Path,
    suffix: &str,
) -> Result<NamedTempFile, MediaError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cache destination parent does not exist",
        )
        .into());
    }
    let temporary_dir = parent.join(".tmp");
    std::fs::create_dir_all(&temporary_dir)?;
    Ok(tempfile::Builder::new()
        .suffix(suffix)
        .tempfile_in(temporary_dir)?)
}

pub(crate) fn write_jpeg_atomically(
    image: &DynamicImage,
    destination: &Path,
    quality: u8,
    contract: ArtifactContract,
) -> Result<(), MediaError> {
    write_jpeg_atomically_cancelled(image, destination, quality, contract, || false)
}

pub(crate) fn write_jpeg_atomically_cancelled(
    image: &DynamicImage,
    destination: &Path,
    quality: u8,
    contract: ArtifactContract,
    cancelled: impl Fn() -> bool,
) -> Result<(), MediaError> {
    write_jpeg_atomically_with_icc(
        image,
        destination,
        quality,
        presentation_icc(contract)?,
        cancelled,
    )
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
    contract: ArtifactContract,
    cancelled: impl Fn() -> bool,
) -> Result<CacheWriteTiming, MediaError> {
    let temporary = cache_tempfile(destination, ".jpg")?;
    if contract.color() != ColorState::SrgbWithIcc {
        return Err(MediaError::Color(
            "decoded JPEG cache requires the sRGB-with-ICC contract".into(),
        ));
    }
    let encode_started = Instant::now();
    #[cfg(target_os = "macos")]
    {
        crate::backends::apple_image_io::write_jpeg(image, temporary.path(), quality)?;
        let profile = presentation_icc(contract)?.expect("sRGB contract has an ICC profile");
        embed_icc_profile(temporary.path(), &profile)?;
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        let mut output = temporary.as_file();
        let mut encoder = JpegEncoder::new_with_quality(&mut output, quality);
        if let Some(profile) = presentation_icc(contract)? {
            encoder
                .set_icc_profile(profile)
                .map_err(|error| MediaError::Color(error.to_string()))?;
        }
        encoder.encode_image(image)?;
    }
    let encode_ms = duration_ms(encode_started);
    let sync_started = Instant::now();
    temporary.as_file().sync_all()?;
    let sync_ms = duration_ms(sync_started);
    if cancelled() {
        return Err(MediaError::Cancelled);
    }
    let commit_started = Instant::now();
    persist_noclobber(temporary, destination)?;
    Ok(CacheWriteTiming {
        encode_ms,
        sync_ms,
        commit_ms: duration_ms(commit_started),
    })
}

#[cfg(target_os = "macos")]
fn embed_icc_profile(path: &Path, profile: &[u8]) -> Result<(), MediaError> {
    const MAX_CHUNK: usize = u16::MAX as usize - 16;
    let jpeg = fs::read(path)?;
    if !jpeg.starts_with(&[0xff, 0xd8]) {
        return Err(MediaError::Color("ImageIO produced an invalid JPEG".into()));
    }
    let chunk_count = profile.len().div_ceil(MAX_CHUNK);
    let chunk_count = u8::try_from(chunk_count)
        .map_err(|_| MediaError::Color("ICC profile requires too many JPEG chunks".into()))?;
    let mut output = Vec::with_capacity(jpeg.len() + profile.len() + 18 * usize::from(chunk_count));
    output.extend_from_slice(&jpeg[..2]);
    for (index, chunk) in profile.chunks(MAX_CHUNK).enumerate() {
        output.extend_from_slice(&[0xff, 0xe2]);
        let length = u16::try_from(chunk.len() + 16)
            .map_err(|_| MediaError::Color("ICC JPEG chunk is too large".into()))?;
        output.extend_from_slice(&length.to_be_bytes());
        output.extend_from_slice(b"ICC_PROFILE\0");
        output.push(u8::try_from(index + 1).unwrap_or(u8::MAX));
        output.push(chunk_count);
        output.extend_from_slice(chunk);
    }
    output.extend_from_slice(&jpeg[2..]);
    fs::write(path, output)?;
    Ok(())
}

fn presentation_icc(contract: ArtifactContract) -> Result<Option<Vec<u8>>, MediaError> {
    match contract.color() {
        ColorState::SrgbWithIcc => lcms2::Profile::new_srgb()
            .icc()
            .map(Some)
            .map_err(|error| MediaError::Color(error.to_string())),
        ColorState::EmbeddedProfileOrUnknown => Ok(None),
    }
}

/// Encode `image` as JPEG, optionally embedding an ICC profile in an APP2 chunk.
/// Used by the unified cache layer so every preview stage and format lands as a
/// color-managed JPEG on disk.
fn write_jpeg_atomically_with_icc(
    image: &DynamicImage,
    destination: &Path,
    quality: u8,
    icc: Option<Vec<u8>>,
    cancelled: impl Fn() -> bool,
) -> Result<(), MediaError> {
    let mut temporary = cache_tempfile(destination, ".jpg")?;
    let mut encoder = JpegEncoder::new_with_quality(&mut temporary, quality);
    if let Some(profile) = icc {
        encoder
            .set_icc_profile(profile)
            .map_err(|error| MediaError::Color(error.to_string()))?;
    }
    encoder.encode_image(image)?;
    temporary.as_file().sync_all()?;
    if cancelled() {
        return Err(MediaError::Cancelled);
    }
    persist_noclobber(temporary, destination)
}

pub(crate) fn write_bytes_atomically(data: &[u8], destination: &Path) -> Result<(), MediaError> {
    write_bytes_atomically_cancelled(data, destination, || false)
}

pub(crate) fn write_bytes_atomically_cancelled(
    data: &[u8],
    destination: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<(), MediaError> {
    let mut temporary = cache_tempfile(destination, ".tmp")?;
    temporary.write_all(data)?;
    temporary.as_file().sync_all()?;
    if cancelled() {
        return Err(MediaError::Cancelled);
    }
    persist_noclobber(temporary, destination)
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

    fn direct_file_count(path: &Path) -> usize {
        fs::read_dir(path)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .count()
    }

    #[test]
    fn byte_writes_preserve_existing_artifact_and_remove_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("preview.jpg");
        write_bytes_atomically(b"first", &destination).unwrap();
        write_bytes_atomically(b"second", &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"first");
        assert_eq!(direct_file_count(directory.path()), 1);
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
        assert!(destination.is_dir());
        assert_eq!(direct_file_count(directory.path()), 0);
    }

    #[test]
    fn missing_parent_is_not_created_by_writer() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("missing");
        assert!(write_bytes_atomically(b"image", &parent.join("preview.jpg")).is_err());
        assert!(!parent.exists());
    }

    #[test]
    fn cancellation_after_encode_skips_cache_commit() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("cancelled.jpg");
        let error = write_jpeg_atomically_timed(
            &DynamicImage::new_rgb8(32, 16),
            &destination,
            90,
            crate::presentation::RAW_DEVELOPED_JPEG,
            || true,
        )
        .unwrap_err();

        assert!(matches!(error, MediaError::Cancelled));
        assert!(!destination.exists());
        assert_eq!(direct_file_count(directory.path()), 0);
    }

    #[test]
    fn jpeg_writers_produce_decodable_images_without_overwriting() {
        let directory = tempfile::tempdir().unwrap();
        for timed in [false, true] {
            let destination = directory.path().join(format!("{timed}.jpg"));
            let image = DynamicImage::new_rgb8(32, 16);
            if timed {
                write_jpeg_atomically_timed(
                    &image,
                    &destination,
                    90,
                    crate::presentation::RAW_DEVELOPED_JPEG,
                    || false,
                )
                .unwrap();
            } else {
                write_jpeg_atomically(
                    &image,
                    &destination,
                    90,
                    crate::presentation::RAW_DEVELOPED_JPEG,
                )
                .unwrap();
            }
            assert_eq!(image::image_dimensions(&destination).unwrap(), (32, 16));
            let mut decoder = ImageReader::open(&destination)
                .unwrap()
                .into_decoder()
                .unwrap();
            assert!(decoder.icc_profile().unwrap().is_some());
            let original = fs::read(&destination).unwrap();
            let replacement = DynamicImage::new_rgb8(8, 8);
            if timed {
                write_jpeg_atomically_timed(
                    &replacement,
                    &destination,
                    90,
                    crate::presentation::RAW_DEVELOPED_JPEG,
                    || false,
                )
                .unwrap();
            } else {
                write_jpeg_atomically(
                    &replacement,
                    &destination,
                    90,
                    crate::presentation::RAW_DEVELOPED_JPEG,
                )
                .unwrap();
            }
            assert_eq!(fs::read(&destination).unwrap(), original);
        }
        assert_eq!(direct_file_count(directory.path()), 2);
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
            || false,
        )
        .unwrap();
        let mut decoder = ImageReader::open(&destination)
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.icc_profile().unwrap(), Some(profile));
    }
}
