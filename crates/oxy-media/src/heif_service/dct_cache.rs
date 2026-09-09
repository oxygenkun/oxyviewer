//! Safe ownership boundary for the C coefficient stitcher.
use super::{HeifTileData, tile_cache::PositionedTile};
use crate::MediaError;
use std::{
    ffi::{CStr, c_char, c_void},
    fs::File,
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[repr(C)]
struct InputTile {
    data: *const u8,
    length: usize,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}
#[repr(C)]
#[derive(Default, Debug)]
struct Stats {
    read_ms: f64,
    copy_ms: f64,
    write_ms: f64,
    coefficient_bytes: u64,
}
unsafe extern "C" {
    fn oxy_jpeg_stitch(
        tiles: *const InputTile,
        count: usize,
        width: u32,
        height: u32,
        memory_budget: u64,
        context: *mut c_void,
        cancelled: extern "C" fn(*mut c_void) -> i32,
        write: extern "C" fn(*mut c_void, *const u8, usize) -> i32,
        stats: *mut Stats,
        message: *mut c_char,
        message_size: usize,
    ) -> i32;
}
struct Context<'a> {
    file: File,
    cancelled: &'a AtomicBool,
    error: Option<std::io::Error>,
}
extern "C" fn cancelled(context: *mut c_void) -> i32 {
    // SAFETY: the synchronous call retains this uniquely borrowed context until return.
    let context = unsafe { &*(context.cast::<Context<'_>>()) };
    i32::from(context.cancelled.load(Ordering::Acquire))
}
extern "C" fn write(context: *mut c_void, bytes: *const u8, length: usize) -> i32 {
    // SAFETY: C calls synchronously with its live output buffer and exclusive context.
    let context = unsafe { &mut *(context.cast::<Context<'_>>()) };
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
    match context.file.write_all(bytes) {
        Ok(()) => 0,
        Err(error) => {
            context.error = Some(error);
            1
        }
    }
}
/// Returns false only for unsupported inputs. Caller may use the pixel
/// baseline; cancellation and I/O failures must propagate without fallback.
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
            Some(InputTile {
                data: bytes.as_ptr(),
                length: bytes.len(),
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
    let mut context = Context {
        file: File::create(destination)?,
        cancelled: cancel,
        error: None,
    };
    let mut stats = Stats::default();
    let mut message = [0 as c_char; 256];
    // SAFETY: all pointers and Arc input buffers remain live for this synchronous
    // call. C catches its own longjmp; callbacks return normally before C unwinds.
    let status = unsafe {
        oxy_jpeg_stitch(
            inputs.as_ptr(),
            inputs.len(),
            width,
            height,
            256 * 1024 * 1024,
            (&mut context as *mut Context<'_>).cast(),
            cancelled,
            write,
            &mut stats,
            message.as_mut_ptr(),
            message.len(),
        )
    };
    // SAFETY: zero-initialized buffer is always NUL-terminated by the C adapter.
    let reason = unsafe { CStr::from_ptr(message.as_ptr()) }.to_string_lossy();
    match status {
        0 => {
            eprintln!("HEIF DCT cache: {stats:?}");
            Ok(true)
        }
        1 => {
            eprintln!("HEIF DCT fallback: {reason}");
            Ok(false)
        }
        2 => Err(MediaError::Cancelled),
        _ => match context.error {
            Some(error) => Err(error.into()),
            None => Err(MediaError::CacheArtifact(format!(
                "JPEG stitch failed: {reason}"
            ))),
        },
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
            let mut tiles =
                crate::backends::ffmpeg_heif::decode_full_jpeg_tiles(&fixture, dimensions, sharpen)
                    .unwrap()
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

    struct Probe {
        output: Vec<u8>,
        polls: usize,
        cancel_at: usize,
        fail_write: bool,
    }
    extern "C" fn probe_cancel(context: *mut c_void) -> i32 {
        // SAFETY: the probe lives for the synchronous native test call.
        let probe = unsafe { &mut *context.cast::<Probe>() };
        probe.polls += 1;
        i32::from(probe.polls >= probe.cancel_at)
    }
    extern "C" fn probe_write(context: *mut c_void, bytes: *const u8, length: usize) -> i32 {
        // SAFETY: C supplies a live byte slice and exclusive probe pointer.
        let probe = unsafe { &mut *context.cast::<Probe>() };
        if probe.fail_write {
            return 1;
        }
        probe
            .output
            .extend_from_slice(unsafe { std::slice::from_raw_parts(bytes, length) });
        0
    }
    fn probe(
        tile: &PositionedTile,
        budget: u64,
        cancel_at: usize,
        fail_write: bool,
    ) -> (i32, Probe) {
        let HeifTileData::Jpeg(bytes) = &tile.tile.payload else {
            unreachable!()
        };
        let input = InputTile {
            data: bytes.as_ptr(),
            length: bytes.len(),
            x: 0,
            y: 0,
            width: tile.tile.width,
            height: tile.tile.height,
        };
        let mut probe = Probe {
            output: Vec::new(),
            polls: 0,
            cancel_at,
            fail_write,
        };
        let mut stats = Stats::default();
        let mut message = [0 as c_char; 256];
        // SAFETY: all inputs/output/callback state remain live through the call.
        let status = unsafe {
            oxy_jpeg_stitch(
                &input,
                1,
                input.width,
                input.height,
                budget,
                (&mut probe as *mut Probe).cast(),
                probe_cancel,
                probe_write,
                &mut stats,
                message.as_mut_ptr(),
                message.len(),
            )
        };
        (status, probe)
    }
    #[test]
    fn cancellation_budget_corruption_and_io_stay_inside_native_boundary() {
        let tile = tile(0, 0, 256, 256, 90);
        let (status, complete) = probe(&tile, 256 * 1024 * 1024, usize::MAX, false);
        assert_eq!(status, 0);
        // Exercise early header, entropy/copy, and final encoding cancellation.
        for point in [1, 10, complete.polls / 2, complete.polls - 1] {
            assert_eq!(probe(&tile, 256 * 1024 * 1024, point, false).0, 2);
        }
        assert_eq!(probe(&tile, 256 * 1024 * 1024, usize::MAX, true).0, 3);
        assert_eq!(probe(&tile, 1, usize::MAX, false).0, 1);
        let HeifTileData::Jpeg(bytes) = &tile.tile.payload else {
            unreachable!()
        };
        let payload = HeifTileData::Jpeg(Arc::from(&bytes[..bytes.len() / 2]));
        let mut truncated = tile;
        truncated.tile.payload = payload;
        assert_eq!(probe(&truncated, 256 * 1024 * 1024, usize::MAX, false).0, 4);
    }
}
