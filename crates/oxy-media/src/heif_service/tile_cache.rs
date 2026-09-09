//! Background-only assembly of the exact tiles published by a display session.
//! Pixel fallback for payloads incompatible with the DCT coefficient stitcher.

use super::{HeifTile, HeifTileData};
use crate::MediaError;
use image::{DynamicImage, RgbaImage};
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_ASSEMBLY_BYTES: u64 = 256 * 1024 * 1024;

pub(super) struct PositionedTile {
    pub x: u32,
    pub y: u32,
    pub tile: HeifTile,
}

fn invalid(message: &str) -> MediaError {
    MediaError::CacheArtifact(message.into())
}

pub(super) fn assemble(
    width: u32,
    height: u32,
    tiles: &[PositionedTile],
    cancelled: &AtomicBool,
) -> Result<DynamicImage, MediaError> {
    let check_cancelled = || {
        if cancelled.load(Ordering::Acquire) {
            Err(MediaError::Cancelled)
        } else {
            Ok(())
        }
    };
    check_cancelled()?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > MAX_ASSEMBLY_BYTES / 4 {
        return Err(invalid(
            "HEIF tile assembly exceeds its pixel memory budget",
        ));
    }
    let mut covered = 0_u64;
    for (index, positioned) in tiles.iter().enumerate() {
        let PositionedTile { x, y, tile } = positioned;
        if tile.width == 0
            || tile.height == 0
            || x.checked_add(tile.width).is_none_or(|right| right > width)
            || y.checked_add(tile.height)
                .is_none_or(|bottom| bottom > height)
        {
            return Err(invalid("HEIF tile exceeds the display canvas"));
        }
        for earlier in &tiles[..index] {
            if *x < earlier.x + earlier.tile.width
                && x + tile.width > earlier.x
                && *y < earlier.y + earlier.tile.height
                && y + tile.height > earlier.y
            {
                return Err(invalid("HEIF cache tiles overlap"));
            }
        }
        covered += u64::from(tile.width) * u64::from(tile.height);
    }
    if covered != pixels {
        return Err(invalid(
            "HEIF cache tiles do not cover native display detail",
        ));
    }
    let mut storage = Vec::new();
    let length = (pixels * 4) as usize;
    storage
        .try_reserve_exact(length)
        .map_err(|_| invalid("cannot allocate HEIF tile assembly buffer"))?;
    storage.resize(length, 0);
    let mut image = RgbaImage::from_raw(width, height, storage)
        .ok_or_else(|| invalid("invalid HEIF assembly dimensions"))?;
    for PositionedTile { x, y, tile } in tiles {
        check_cancelled()?;
        let decoded;
        let (stride, bytes) = match &tile.payload {
            HeifTileData::Jpeg(jpeg) => {
                let reader = image::ImageReader::with_format(
                    std::io::Cursor::new(jpeg.as_ref()),
                    image::ImageFormat::Jpeg,
                );
                if reader.into_dimensions()? != (tile.width, tile.height) {
                    return Err(invalid("JPEG tile dimensions differ from its publication"));
                }
                decoded = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)?
                    .into_rgba8();
                (tile.width as usize * 4, decoded.as_raw().as_slice())
            }
            HeifTileData::Rgba { stride, bytes } => (*stride as usize, bytes.as_ref()),
        };
        let row_bytes = tile.width as usize * 4;
        if stride < row_bytes || stride.checked_mul(tile.height as usize) != Some(bytes.len()) {
            return Err(invalid("invalid RGBA tile stride or length"));
        }
        for row in 0..tile.height as usize {
            check_cancelled()?;
            let destination = ((*y as usize + row) * width as usize + *x as usize) * 4;
            image.as_mut()[destination..destination + row_bytes]
                .copy_from_slice(&bytes[row * stride..row * stride + row_bytes]);
        }
    }
    Ok(DynamicImage::ImageRgba8(image))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn tile(x: u32, value: u8) -> PositionedTile {
        PositionedTile {
            x,
            y: 0,
            tile: HeifTile {
                width: 1,
                height: 1,
                payload: HeifTileData::Rgba {
                    stride: 4,
                    bytes: Arc::from([value, value, value, 255]),
                },
            },
        }
    }

    #[test]
    fn assembly_preserves_display_pixels_without_sharpening_again() {
        let tiles = [tile(1, 240), tile(0, 10)];
        let result = assemble(2, 1, &tiles, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            result.into_rgba8().as_raw(),
            &[10, 10, 10, 255, 240, 240, 240, 255]
        );
    }

    #[test]
    fn jpeg_assembly_matches_decoded_display_tile() {
        let source = DynamicImage::ImageRgb8(image::RgbImage::from_fn(8, 8, |x, y| {
            image::Rgb([(x * 20) as u8, (y * 20) as u8, 100])
        }));
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&source)
            .unwrap();
        let expected = image::load_from_memory(&jpeg).unwrap().into_rgba8();
        let tiles = [PositionedTile {
            x: 0,
            y: 0,
            tile: HeifTile {
                width: 8,
                height: 8,
                payload: HeifTileData::Jpeg(Arc::from(jpeg)),
            },
        }];
        assert_eq!(
            assemble(8, 8, &tiles, &AtomicBool::new(false))
                .unwrap()
                .into_rgba8(),
            expected
        );
        assert!(assemble(8, 8, &tiles, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn rejects_incomplete_overlapping_out_of_bounds_and_oversized_assemblies() {
        for (width, height, tiles) in [
            (2, 1, vec![tile(0, 10)]),
            (2, 1, vec![tile(0, 10), tile(0, 20)]),
            (1, 1, vec![tile(u32::MAX, 10)]),
            (u32::MAX, u32::MAX, vec![]),
        ] {
            assert!(assemble(width, height, &tiles, &AtomicBool::new(false)).is_err());
        }
    }
}
