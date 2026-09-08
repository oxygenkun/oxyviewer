use crate::{
    MediaError,
    presentation::{ArtifactContract, ColorState},
};
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, codecs::jpeg::JpegEncoder};
use std::path::Path;
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

/// Attach the explicit sRGB profile to an already color-converted native JPEG.
/// ImageIO may omit ICC for its named sRGB space. Insert APP2 without pixel
/// decoding/re-encoding or a second full-image allocation. The caller owns and
/// leases this unpublished staging file, so a failed write is simply discarded.
pub(crate) fn ensure_srgb_icc(path: &Path) -> Result<(), MediaError> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let has_profile = ImageReader::open(path)?
        .into_decoder()?
        .icc_profile()?
        .is_some();
    if has_profile {
        return Ok(());
    }
    let profile = lcms2::Profile::new_srgb()
        .icc()
        .map_err(|error| MediaError::Color(error.to_string()))?;
    let length = u16::try_from(profile.len() + 16)
        .map_err(|_| MediaError::Color("sRGB ICC profile is too large for JPEG APP2".into()))?;
    let mut segment = Vec::with_capacity(profile.len() + 18);
    segment.extend_from_slice(&[0xff, 0xe2]);
    segment.extend_from_slice(&length.to_be_bytes());
    segment.extend_from_slice(b"ICC_PROFILE\0");
    segment.extend_from_slice(&[1, 1]);
    segment.extend_from_slice(&profile);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut magic = [0; 2];
    file.read_exact(&mut magic)?;
    if magic != [0xff, 0xd8] {
        return Err(MediaError::Color("native sRGB output is not JPEG".into()));
    }
    let mut end = file.metadata()?.len();
    let shift = segment.len() as u64;
    file.set_len(
        end.checked_add(shift)
            .ok_or_else(|| MediaError::Color("JPEG length overflow".into()))?,
    )?;
    let mut buffer = [0_u8; 64 * 1024];
    while end > 2 {
        let count = (end - 2).min(buffer.len() as u64) as usize;
        let start = end - count as u64;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buffer[..count])?;
        file.seek(SeekFrom::Start(start + shift))?;
        file.write_all(&buffer[..count])?;
        end = start;
    }
    file.seek(SeekFrom::Start(2))?;
    file.write_all(&segment)?;
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

fn persist_noclobber(temporary: NamedTempFile, destination: &Path) -> Result<(), MediaError> {
    match temporary.persist_noclobber(destination) {
        Ok(_) => Ok(()),
        Err(_error) if destination.is_file() => Ok(()),
        Err(error) => Err(MediaError::Io(error.error)),
    }
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

    fn assert_no_temporary_files(path: &Path) {
        assert_eq!(fs::read_dir(path.join(".tmp")).unwrap().count(), 0);
    }

    #[test]
    fn failed_commit_keeps_destination_directory_and_cleans_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("preview.jpg");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep"), b"keep").unwrap();

        assert!(matches!(
            write_jpeg_atomically(
                &DynamicImage::new_rgb8(32, 16),
                &destination,
                90,
                crate::presentation::RAW_DEVELOPED_JPEG,
            ),
            Err(MediaError::Io(_))
        ));
        assert_eq!(fs::read(destination.join("keep")).unwrap(), b"keep");
        assert!(destination.is_dir());
        assert_eq!(direct_file_count(directory.path()), 0);
        assert_no_temporary_files(directory.path());
    }

    #[test]
    fn native_profile_attachment_preserves_encoded_bytes_and_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.jpg");
        let mut state = 0x1234_5678_u32;
        let image = image::RgbImage::from_fn(512, 512, |_, _| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let bytes = state.to_le_bytes();
            image::Rgb([bytes[0], bytes[1], bytes[2]])
        });
        let mut original = Vec::new();
        JpegEncoder::new_with_quality(&mut original, 95)
            .encode_image(&image)
            .unwrap();
        assert!(original.len() > 64 * 1024);
        fs::write(&path, &original).unwrap();
        ensure_srgb_icc(&path).unwrap();
        let tagged = fs::read(&path).unwrap();
        let inserted = tagged.len() - original.len();
        assert_eq!(&tagged[inserted + 2..], &original[2..]);
        assert!(
            ImageReader::open(&path)
                .unwrap()
                .into_decoder()
                .unwrap()
                .icc_profile()
                .unwrap()
                .is_some()
        );
        ensure_srgb_icc(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), tagged);
    }

    #[test]
    fn missing_parent_is_not_created_by_writer() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("missing");
        assert!(
            write_jpeg_atomically(
                &DynamicImage::new_rgb8(32, 16),
                &parent.join("preview.jpg"),
                90,
                crate::presentation::RAW_DEVELOPED_JPEG,
            )
            .is_err()
        );
        assert!(!parent.exists());
    }

    #[test]
    fn cancellation_after_encode_skips_cache_commit() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("cancelled.jpg");
        let error = write_jpeg_atomically_cancelled(
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
        assert_no_temporary_files(directory.path());
    }

    #[test]
    fn jpeg_writer_produces_decodable_image_without_overwriting() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("preview.jpg");
        write_jpeg_atomically(
            &DynamicImage::new_rgb8(32, 16),
            &destination,
            90,
            crate::presentation::RAW_DEVELOPED_JPEG,
        )
        .unwrap();
        assert_eq!(image::image_dimensions(&destination).unwrap(), (32, 16));
        let mut decoder = ImageReader::open(&destination)
            .unwrap()
            .into_decoder()
            .unwrap();
        assert!(decoder.icc_profile().unwrap().is_some());
        let original = fs::read(&destination).unwrap();
        write_jpeg_atomically(
            &DynamicImage::new_rgb8(8, 8),
            &destination,
            90,
            crate::presentation::RAW_DEVELOPED_JPEG,
        )
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), original);
        assert_eq!(direct_file_count(directory.path()), 1);
        assert_no_temporary_files(directory.path());
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
