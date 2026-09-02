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
        // Sony stores this thumbnail with the subject's head at the left edge;
        // the display transform is therefore 90 degrees clockwise (EXIF 6).
        jpeg = with_exif_orientation(jpeg, 6);
        std::mem::swap(&mut width, &mut height);
    }
    Ok(EmbeddedJpeg {
        bytes: jpeg,
        width,
        height,
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
    fn extracts_repository_sony_sidebar_jpeg() {
        let path = Path::new("../../tests/fixtures/DSC00449.HIF");
        if !path.is_file() {
            return;
        }
        let image = extract(
            path,
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
