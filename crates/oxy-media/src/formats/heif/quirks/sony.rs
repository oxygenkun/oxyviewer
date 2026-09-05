//! Sony SHIF's bounded embedded-JPEG extraction and orientation compatibility.
//! This is not a generic HEIF thumbnail extractor or a decoder backend.

use crate::{ImageDimensions, MediaError};
use image::GenericImageView;
use std::{fs::File, io::Read, path::Path};

// Sony HIF stores its sidebar JPEG near the start of the BMFF file. Keep the
// scan bounded so a corrupt or unusual file cannot turn thumbnail discovery
// into a full-file read on the browsing path.
const SCAN_LIMIT: u64 = 2 * 1024 * 1024;
const MAX_EDGE: u32 = 512;

pub struct EmbeddedJpeg {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn extract(path: &Path, display_size: ImageDimensions) -> Result<EmbeddedJpeg, MediaError> {
    let mut bytes = Vec::new();
    File::open(path)?.take(SCAN_LIMIT).read_to_end(&mut bytes)?;
    if !bytes.windows(4).any(|window| window == b"SHIF") {
        return Err(MediaError::NativeDecode {
            backend: "embedded JPEG",
            message: format!("{} is not a Sony SHIF container", path.display()),
        });
    }
    let mut best: Option<(u64, Vec<u8>, u32, u32)> = None;
    let mut cursor = 0;
    while let Some(relative_start) = find(&bytes[cursor..], &[0xff, 0xd8, 0xff]) {
        let start = cursor + relative_start;
        let Some(relative_end) = find(&bytes[start + 3..], &[0xff, 0xd9]) else {
            cursor = start + 3;
            continue;
        };
        let end = start + 3 + relative_end + 2;
        if let Ok(image) = image::load_from_memory(&bytes[start..end]) {
            let (width, height) = image.dimensions();
            if width >= 32 && height >= 32 && width.max(height) <= MAX_EDGE {
                let area = u64::from(width) * u64::from(height);
                if best
                    .as_ref()
                    .is_none_or(|(best_area, _, _, _)| area > *best_area)
                {
                    best = Some((area, bytes[start..end].to_vec(), width, height));
                }
            }
        }
        // Keep searching from the marker instead of the candidate end. EXIF
        // blobs may contain an outer JPEG-like marker range that encloses the
        // actual HEIF item; jumping to its EOI would skip the valid image.
        cursor = start + 3;
    }
    let (_, mut jpeg, mut width, mut height) = best.ok_or_else(|| MediaError::NativeDecode {
        backend: "embedded JPEG",
        message: format!("{} has no small embedded JPEG", path.display()),
    })?;
    let image_is_portrait = height > width;
    let display_is_portrait = display_size.height > display_size.width;
    if image_is_portrait != display_is_portrait {
        // The JPEG item itself has no orientation tag. Apply the primary HEIF
        // item's irot transform so the fast thumbnail agrees with libheif's
        // display-oriented decode. Sony writes both clockwise and
        // counter-clockwise portrait captures, so aspect ratio alone cannot
        // choose between EXIF 6 and 8.
        let orientation = heif_exif_orientation(&bytes).unwrap_or(6);
        jpeg = with_exif_orientation(jpeg, orientation);
        std::mem::swap(&mut width, &mut height);
    }
    Ok(EmbeddedJpeg {
        bytes: jpeg,
        width,
        height,
    })
}

fn heif_exif_orientation(bytes: &[u8]) -> Option<u16> {
    bytes.windows(9).find_map(|window| {
        let size = u32::from_be_bytes(window[..4].try_into().ok()?);
        (size == 9 && &window[4..8] == b"irot").then(|| match window[8] & 0x03 {
            // HEIF irot stores counter-clockwise quarter turns, while EXIF
            // orientation names the clockwise display operation.
            1 => 8,
            2 => 3,
            3 => 6,
            _ => 1,
        })
    })
}

fn with_exif_orientation(jpeg: Vec<u8>, orientation: u16) -> Vec<u8> {
    // Minimal little-endian TIFF containing only EXIF Orientation. Browsers
    // apply it during JPEG decode, giving us lossless rotation without paying
    // for a decode/re-encode cycle on every scrolling thumbnail.
    const PREFIX: [u8; 24] = [
        0xff, 0xe1, 0x00, 0x22, b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 0x2a, 0, 8, 0, 0, 0, 1,
        0, 0x12, 0x01, 3, 0,
    ];
    let mut result = Vec::with_capacity(jpeg.len() + 36);
    result.extend_from_slice(&jpeg[..2]);
    result.extend_from_slice(&PREFIX);
    result.extend_from_slice(&[1, 0, 0, 0]);
    result.extend_from_slice(&orientation.to_le_bytes());
    result.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    result.extend_from_slice(&jpeg[2..]);
    result
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_heif_rotation_to_exif_orientation() {
        for (quarter_turns, expected) in [(0, 1), (1, 8), (2, 3), (3, 6)] {
            let mut bytes = vec![0, 0, 0, 9];
            bytes.extend_from_slice(b"irot");
            bytes.push(quarter_turns);
            assert_eq!(heif_exif_orientation(&bytes), Some(expected));
        }
    }

    #[test]
    fn rejects_files_without_sony_signature() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("generic.hif");
        std::fs::write(&path, b"not a Sony container").unwrap();
        let error = extract(
            &path,
            ImageDimensions {
                width: 160,
                height: 120,
            },
        )
        .err()
        .expect("non-Sony input must not use the quirk");
        assert!(matches!(error, MediaError::NativeDecode { message, .. }
            if message.contains("not a Sony SHIF container")));
    }

    #[test]
    fn sony_signature_without_jpeg_is_not_a_preview() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("empty.hif");
        std::fs::write(&path, b"SHIF").unwrap();
        let error = extract(
            &path,
            ImageDimensions {
                width: 160,
                height: 120,
            },
        )
        .err()
        .expect("signature alone cannot supply a preview");
        assert!(matches!(error, MediaError::NativeDecode { message, .. }
            if message.contains("no small embedded JPEG")));
    }

    #[test]
    fn extracts_repository_sony_sidebar_jpeg() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        let image = extract(
            &path,
            ImageDimensions {
                width: 4_672,
                height: 7_008,
            },
        )
        .unwrap();
        assert_eq!((image.width, image.height), (120, 160));
        assert_eq!(&image.bytes[..2], &[0xff, 0xd8]);
        assert_eq!(&image.bytes[2..4], &[0xff, 0xe1]);
        assert_eq!(&image.bytes[30..32], &[6, 0]);
    }
}
