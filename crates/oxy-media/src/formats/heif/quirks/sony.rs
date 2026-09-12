//! Sony SHIF's bounded embedded-JPEG extraction and orientation compatibility.
//! This is not a generic HEIF thumbnail extractor or a decoder backend.

mod geometry;

use crate::{ImageDimensions, MediaError};
use image::GenericImageView;
use std::{fs::File, io::Read, path::Path};

// Sony HIF stores its sidebar JPEG near the start of the BMFF file. Keep the
// scan bounded so a corrupt or unusual file cannot turn thumbnail discovery
// into a full-file read on the browsing path.
const SCAN_LIMIT: u64 = 2 * 1024 * 1024;
const INITIAL_SCAN_LIMIT: u64 = 256 * 1024;
const MAX_EDGE: u32 = 512;

#[derive(Debug)]
pub struct EmbeddedJpeg {
    pub facts: oxy_domain::ArtifactFacts,
    pub geometry: Option<oxy_domain::PreviewGeometry>,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct Inspection {
    pub embedded_jpeg: Option<EmbeddedJpeg>,
}

/// Inspect only the bounded SHIF header area and retain a discovered fast
/// origin so planning and execution do not read the source twice.
pub fn inspect(
    path: &Path,
    display_size: Option<ImageDimensions>,
) -> Result<Inspection, MediaError> {
    inspect_reader(File::open(path)?, display_size)
}

fn inspect_reader(
    mut reader: impl Read,
    display_size: Option<ImageDimensions>,
) -> Result<Inspection, MediaError> {
    // Sony's sidebar JPEG normally fits in this prefix. Reading the full
    // 2 MiB for every 160px thumbnail needlessly competes with visible work
    // on SMB. Keep the original bounded fallback for later/incomplete JPEGs.
    let mut bytes = Vec::with_capacity(INITIAL_SCAN_LIMIT as usize);
    reader
        .by_ref()
        .take(INITIAL_SCAN_LIMIT)
        .read_to_end(&mut bytes)?;
    let first = inspect_bytes(&bytes, display_size);
    if (first.embedded_jpeg.is_some() && has_complete_metadata_prefix(&bytes))
        || bytes.len() < INITIAL_SCAN_LIMIT as usize
    {
        return Ok(first);
    }
    reader
        .take(SCAN_LIMIT - INITIAL_SCAN_LIMIT)
        .read_to_end(&mut bytes)?;
    Ok(inspect_bytes(&bytes, display_size))
}

// Only stop early when the complete top-level meta box is present, so later
// orientation/dimension properties cannot change how the JPEG is displayed.
fn has_complete_metadata_prefix(bytes: &[u8]) -> bool {
    let mut offset = 0_usize;
    while let Some(header) = bytes.get(offset..).and_then(|tail| tail.get(..8)) {
        let size = u32::from_be_bytes(header[..4].try_into().unwrap());
        let (size, header_size) = match size {
            0 => return false,
            1 => {
                let Some(extended) = bytes.get(offset + 8..offset + 16) else {
                    return false;
                };
                (u64::from_be_bytes(extended.try_into().unwrap()), 16)
            }
            size => (u64::from(size), 8),
        };
        let Some(end) = usize::try_from(size)
            .ok()
            .and_then(|size| offset.checked_add(size))
        else {
            return false;
        };
        if size < header_size || end > bytes.len() {
            return false;
        }
        if &header[4..8] == b"meta" {
            return true;
        }
        offset = end;
    }
    false
}

fn inspect_bytes(bytes: &[u8], display_size: Option<ImageDimensions>) -> Inspection {
    if !bytes.windows(4).any(|window| window == b"SHIF") {
        return Inspection {
            embedded_jpeg: None,
        };
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
    let Some((_, mut jpeg, mut width, mut height)) = best else {
        return Inspection {
            embedded_jpeg: None,
        };
    };
    let primary = geometry::primary(bytes);
    let orientation = primary.map_or_else(
        || heif_exif_orientation(bytes).unwrap_or(1),
        |(_, turn)| match turn {
            1 => 8,
            2 => 3,
            3 => 6,
            _ => 1,
        },
    );
    let geometry = primary.and_then(|(size, turn)| {
        image::load_from_memory(&jpeg)
            .ok()
            .and_then(|image| geometry::detect(&image, size, turn))
    });
    let needs_axis_swap = matches!(orientation, 6 | 8);
    let display_size = display_size.or_else(|| {
        heif_display_dimensions(bytes).map(|mut dimensions| {
            if needs_axis_swap {
                std::mem::swap(&mut dimensions.width, &mut dimensions.height);
            }
            dimensions
        })
    });
    let known_landscape_jpeg =
        (width, height) == (160, 120) && primary.is_some_and(|(size, _)| size.width >= size.height);
    let needs_orientation = if known_landscape_jpeg {
        orientation != 1
    } else {
        display_size.map_or(needs_axis_swap, |display_size| {
            (height > width) != (display_size.height > display_size.width)
        })
    };
    use sha2::{Digest, Sha256};
    let candidate_id = format!("sony-jpeg:{:x}", Sha256::digest(&jpeg));
    let encoded_dimensions = oxy_domain::EncodedDimensions((width, height).into());
    let output_orientation = if needs_orientation {
        if orientation != 1 { orientation } else { 6 }
    } else {
        1
    };
    if needs_orientation {
        // The JPEG item itself has no orientation tag. Apply the primary HEIF
        // item's irot transform so the fast thumbnail agrees with libheif's
        // display-oriented decode. Sony writes both clockwise and
        // counter-clockwise portrait captures, so aspect ratio alone cannot
        // choose between EXIF 6 and 8.
        let orientation = if orientation != 1 { orientation } else { 6 };
        jpeg = with_exif_orientation(jpeg, orientation);
        if matches!(orientation, 6 | 8) {
            std::mem::swap(&mut width, &mut height);
        }
    }
    let reference = geometry.map_or_else(
        || display_size.unwrap_or(ImageDimensions { width, height }),
        |geometry| ImageDimensions {
            width: geometry.display_size.width,
            height: geometry.display_size.height,
        },
    );
    let mut facts = crate::media_source::source_facts(
        oxy_domain::ImageOrigin::EmbeddedPreview,
        candidate_id,
        encoded_dimensions,
        output_orientation as u8,
        oxy_domain::DisplayDimensions(reference),
    );
    facts.byte_integrity = if needs_orientation {
        facts
            .processing
            .push(oxy_domain::ImageOperation::MetadataEdit);
        oxy_domain::ByteIntegrity::MetadataAdjusted
    } else {
        oxy_domain::ByteIntegrity::SourcePayload
    };
    Inspection {
        embedded_jpeg: Some(EmbeddedJpeg {
            facts,
            geometry,
            bytes: jpeg,
        }),
    }
}

#[cfg(test)]
fn extract(path: &Path, display_size: ImageDimensions) -> Result<EmbeddedJpeg, MediaError> {
    let inspection = inspect(path, Some(display_size))?;
    inspection
        .embedded_jpeg
        .ok_or_else(|| MediaError::NativeDecode {
            backend: "embedded JPEG",
            message: format!("{} has no small embedded JPEG", path.display()),
        })
}

fn heif_display_dimensions(bytes: &[u8]) -> Option<ImageDimensions> {
    bytes
        .windows(20)
        .filter_map(|window| {
            let size = u32::from_be_bytes(window[..4].try_into().ok()?);
            if size != 20 || &window[4..8] != b"ispe" {
                return None;
            }
            let width = u32::from_be_bytes(window[12..16].try_into().ok()?);
            let height = u32::from_be_bytes(window[16..20].try_into().ok()?);
            (width > 0 && height > 0).then_some(ImageDimensions { width, height })
        })
        .max_by_key(|dimensions| u64::from(dimensions.width) * u64::from(dimensions.height))
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
    use std::io::Cursor;

    fn header_with_jpeg(offset: usize) -> Vec<u8> {
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(160, 120)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let mut bytes = vec![0; SCAN_LIMIT as usize + 1];
        bytes[..20].copy_from_slice(b"\0\0\0\x14ftypheic\0\0\0\0SHIF");
        bytes[20..32].copy_from_slice(b"\0\0\0\x15meta\0\0\0\0");
        // Portrait metadata must survive the short-prefix path unchanged.
        bytes[32..41].copy_from_slice(&[0, 0, 0, 9, b'i', b'r', b'o', b't', 3]);
        let jpeg = jpeg.into_inner();
        bytes[offset..offset + jpeg.len()].copy_from_slice(&jpeg);
        bytes
    }

    #[test]
    fn stops_reading_after_a_valid_jpeg_in_the_small_prefix() {
        let mut reader = Cursor::new(header_with_jpeg(155_648));
        let image = inspect_reader(&mut reader, None)
            .unwrap()
            .embedded_jpeg
            .unwrap();
        assert_eq!(reader.position(), INITIAL_SCAN_LIMIT);
        assert_eq!(
            (
                image.facts.display_dimensions.0.width,
                image.facts.display_dimensions.0.height
            ),
            (120, 160)
        );
        assert_eq!(&image.bytes[30..32], &[6, 0]);
    }

    #[test]
    fn incomplete_or_later_jpeg_extends_the_read_to_the_original_bound() {
        for offset in [INITIAL_SCAN_LIMIT as usize - 3, 512 * 1024] {
            let mut reader = Cursor::new(header_with_jpeg(offset));
            let image = inspect_reader(&mut reader, None)
                .unwrap()
                .embedded_jpeg
                .unwrap();
            assert_eq!(reader.position(), SCAN_LIMIT);
            assert_eq!(
                (
                    image.facts.display_dimensions.0.width,
                    image.facts.display_dimensions.0.height
                ),
                (120, 160)
            );
        }
    }

    #[test]
    fn incomplete_metadata_extends_the_read_even_when_jpeg_is_already_present() {
        let mut bytes = header_with_jpeg(155_648);
        bytes[20..24].copy_from_slice(&400_000_u32.to_be_bytes());
        bytes[32..41].fill(0);
        bytes[300_000..300_009].copy_from_slice(&[0, 0, 0, 9, b'i', b'r', b'o', b't', 3]);
        let mut reader = Cursor::new(bytes);
        let image = inspect_reader(&mut reader, None)
            .unwrap()
            .embedded_jpeg
            .unwrap();
        assert_eq!(reader.position(), SCAN_LIMIT);
        assert_eq!(
            (
                image.facts.display_dimensions.0.width,
                image.facts.display_dimensions.0.height
            ),
            (120, 160)
        );
    }

    #[test]
    fn reads_largest_bounded_ispe_as_display_dimensions() {
        let mut bytes = Vec::new();
        for (width, height) in [(512_u32, 512_u32), (4_672, 7_008)] {
            bytes.extend_from_slice(&20_u32.to_be_bytes());
            bytes.extend_from_slice(b"ispe");
            bytes.extend_from_slice(&0_u32.to_be_bytes());
            bytes.extend_from_slice(&width.to_be_bytes());
            bytes.extend_from_slice(&height.to_be_bytes());
        }
        assert_eq!(
            heif_display_dimensions(&bytes),
            Some(ImageDimensions {
                width: 4_672,
                height: 7_008,
            })
        );
    }

    #[test]
    fn fallback_portrait_rotation_reports_the_rotated_raster_dimensions() {
        let mut bytes = header_with_jpeg(155_648);
        bytes[32..41].fill(0); // No usable primary transform: legacy fallback.
        let image = inspect_bytes(
            &bytes,
            Some(ImageDimensions {
                width: 4672,
                height: 7008,
            }),
        )
        .embedded_jpeg
        .unwrap();
        assert_eq!(
            (
                image.facts.display_dimensions.0.width,
                image.facts.display_dimensions.0.height
            ),
            (120, 160)
        );
        assert_eq!(&image.bytes[30..32], &[6, 0]);
        assert!(image.geometry.is_none());
    }

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
        let inspection = inspect(&path, None).unwrap();
        assert!(inspection.embedded_jpeg.is_none());
    }

    #[test]
    fn inspection_does_not_scan_past_the_bounded_header_window() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("late-shif.heic");
        let mut bytes = vec![0; SCAN_LIMIT as usize + 4];
        bytes[SCAN_LIMIT as usize..].copy_from_slice(b"SHIF");
        std::fs::write(&path, bytes).unwrap();

        let inspection = inspect(&path, None).unwrap();
        assert!(inspection.embedded_jpeg.is_none());
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
        .expect_err("signature alone cannot supply a preview");
        assert!(matches!(error, MediaError::NativeDecode { message, .. }
            if message.contains("no small embedded JPEG")));
    }

    #[test]
    fn extracts_repository_sony_sidebar_jpeg() {
        let Some(path) = crate::sony_hif_fixture() else {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        };
        let image = extract(
            &path,
            ImageDimensions {
                width: 4_672,
                height: 7_008,
            },
        )
        .unwrap();
        let mut full_prefix = Vec::new();
        File::open(&path)
            .unwrap()
            .take(SCAN_LIMIT)
            .read_to_end(&mut full_prefix)
            .unwrap();
        let full_scan = inspect_bytes(
            &full_prefix,
            Some(ImageDimensions {
                width: 4_672,
                height: 7_008,
            }),
        )
        .embedded_jpeg
        .unwrap();
        assert_eq!(image.bytes, full_scan.bytes);
        assert_eq!(
            (
                image.facts.display_dimensions.0.width,
                image.facts.display_dimensions.0.height
            ),
            (120, 160)
        );
        assert_eq!(&image.bytes[..2], &[0xff, 0xd8]);
        assert_eq!(&image.bytes[2..4], &[0xff, 0xe1]);
        assert_eq!(&image.bytes[30..32], &[6, 0]);
    }
}
