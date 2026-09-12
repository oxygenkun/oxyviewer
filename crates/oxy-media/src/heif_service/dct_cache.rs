//! HEIF tile adaptation and file ownership for coefficient-cache generation.
use super::{HeifTileData, tile_cache::PositionedTile};
use crate::{
    MediaError,
    backends::libjpeg::stitch::{JpegTile, StitchOutcome, stitch_coefficients},
    media_source::PixelDimensions,
};
use std::{
    fs::File,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// Returns false only for unsupported inputs; cancellation and I/O propagate.
pub(super) fn write_tiles(
    width: u32,
    height: u32,
    tiles: &[PositionedTile],
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<bool, MediaError> {
    if cancel.load(Ordering::Acquire) {
        return Err(MediaError::Cancelled);
    }
    let Some(inputs) = tiles
        .iter()
        .map(|positioned| {
            let HeifTileData::Jpeg(bytes) = &positioned.tile.payload else {
                return None;
            };
            Some(JpegTile {
                bytes,
                x: positioned.x,
                y: positioned.y,
                width: positioned.tile.width,
                height: positioned.tile.height,
            })
        })
        .collect::<Option<Vec<_>>>()
    else {
        return Ok(false);
    };
    let mut file = File::create(destination)?;
    match stitch_coefficients(
        PixelDimensions { width, height },
        &inputs,
        256 * 1024 * 1024,
        &mut file,
        &|| cancel.load(Ordering::Acquire),
    )? {
        StitchOutcome::Stitched(stats) => {
            eprintln!("HEIF DCT cache: {stats:?}");
            Ok(true)
        }
        StitchOutcome::Unsupported(reason) => {
            eprintln!("HEIF DCT fallback: {reason}");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heif_service::HeifTile;
    use std::sync::Arc;

    fn tile(x: u32, y: u32, width: u32, height: u32, quality: u8) -> PositionedTile {
        let pixels = image::RgbImage::from_fn(width, height, |a, b| {
            image::Rgb([
                ((a + x) * 13 % 256) as u8,
                ((b + y) * 17 % 256) as u8,
                ((a + b) * 7 % 256) as u8,
            ])
        });
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(&image::DynamicImage::ImageRgb8(pixels))
            .unwrap();
        PositionedTile {
            x,
            y,
            tile: HeifTile {
                width,
                height,
                payload: HeifTileData::Jpeg(Arc::from(bytes)),
            },
        }
    }

    #[test]
    fn stitches_pixels_with_partial_outer_edges_and_reordered_tiles() {
        let tiles = [
            tile(16, 16, 13, 11, 90),
            tile(0, 0, 16, 16, 90),
            tile(16, 0, 13, 16, 90),
            tile(0, 16, 16, 11, 90),
        ];
        let out = tempfile::NamedTempFile::new().unwrap();
        assert!(write_tiles(29, 27, &tiles, out.path(), &AtomicBool::new(false)).unwrap());
        let actual = image::ImageReader::open(out.path())
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap()
            .into_rgba8();
        let expected = super::super::tile_cache::assemble(29, 27, &tiles, &AtomicBool::new(false))
            .unwrap()
            .into_rgba8();
        assert_eq!(actual, expected);
    }

    #[test]
    fn real_ffmpeg_tiles_preserve_pixels_with_and_without_sharpening() {
        let Some(fixture) = crate::sony_hif_fixture() else {
            eprintln!("skipping: set OXY_HIF_FIXTURE to verify real JPEG tiles");
            return;
        };
        if crate::backends::ffmpeg_heif::capability().is_err() {
            return;
        }
        let dimensions = crate::backends::libheif::dimensions(&fixture).unwrap();
        let directory = tempfile::tempdir().unwrap();
        for sharpen in [false, true] {
            let mut decoded_tiles = Vec::new();
            crate::backends::ffmpeg_heif::decode_full_jpeg_tiles(
                &fixture,
                dimensions,
                sharpen,
                &|| false,
                |tile| {
                    decoded_tiles.push(tile);
                    Ok(())
                },
            )
            .unwrap();
            let mut tiles = decoded_tiles
                .into_iter()
                .map(|tile| PositionedTile {
                    x: tile.x,
                    y: tile.y,
                    tile: HeifTile {
                        width: tile.width,
                        height: tile.height,
                        payload: HeifTileData::Jpeg(Arc::from(tile.jpeg)),
                    },
                })
                .collect::<Vec<_>>();
            tiles.reverse();
            let path = directory.path().join("stitched.jpg");
            assert!(
                write_tiles(
                    dimensions.width,
                    dimensions.height,
                    &tiles,
                    &path,
                    &AtomicBool::new(false)
                )
                .unwrap()
            );
            let actual = image::open(&path).unwrap().into_rgb8();
            for tile in &tiles {
                let HeifTileData::Jpeg(bytes) = &tile.tile.payload else {
                    unreachable!()
                };
                let decoded = image::load_from_memory(bytes).unwrap().into_rgb8();
                for row in 0..tile.tile.height {
                    let offset = ((row + tile.y) * dimensions.width + tile.x) as usize * 3;
                    let input = row as usize * tile.tile.width as usize * 3;
                    let length = tile.tile.width as usize * 3;
                    assert_eq!(
                        &actual.as_raw()[offset..offset + length],
                        &decoded.as_raw()[input..input + length]
                    );
                }
            }
        }
    }

    #[test]
    fn rejects_incompatible_or_incomplete_tiles() {
        let cases = [
            vec![tile(0, 0, 16, 16, 90)],
            vec![tile(0, 0, 16, 16, 90), tile(0, 0, 16, 16, 90)],
            vec![tile(0, 0, 16, 16, 90), tile(16, 0, 16, 16, 80)],
            vec![tile(0, 0, 15, 16, 90), tile(15, 0, 17, 16, 90)],
        ];
        for tiles in cases {
            let out = tempfile::NamedTempFile::new().unwrap();
            assert!(!matches!(
                write_tiles(32, 16, &tiles, out.path(), &AtomicBool::new(false)),
                Ok(true)
            ));
            assert_eq!(out.as_file().metadata().unwrap().len(), 0);
        }
        let mut corrupt = tile(0, 0, 32, 16, 90);
        corrupt.tile.payload = HeifTileData::Jpeg(Arc::from(&b"not jpeg"[..]));
        let out = tempfile::NamedTempFile::new().unwrap();
        assert!(write_tiles(32, 16, &[corrupt], out.path(), &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn rejects_chroma_subsampling_before_reading_coefficients() {
        let mut tile = tile(0, 0, 16, 16, 90);
        let HeifTileData::Jpeg(bytes) = &tile.tile.payload else {
            unreachable!()
        };
        let mut bytes = bytes.to_vec();
        let sof = bytes
            .windows(2)
            .position(|bytes| bytes == [0xff, 0xc0])
            .unwrap();
        // A legal 4:2:2 SOF header; the untouched entropy payload must not be read.
        bytes[sof + 11] = 0x21;
        tile.tile.payload = HeifTileData::Jpeg(Arc::from(bytes));
        let out = tempfile::NamedTempFile::new().unwrap();
        assert!(!write_tiles(16, 16, &[tile], out.path(), &AtomicBool::new(false)).unwrap());
        assert_eq!(out.as_file().metadata().unwrap().len(), 0);
    }
}
