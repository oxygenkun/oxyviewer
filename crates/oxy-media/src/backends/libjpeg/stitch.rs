//! Safe libjpeg coefficient stitching; callers own files and cache publication.
use crate::{MediaError, media_source::PixelDimensions};
use std::{
    ffi::{CStr, c_char, c_void},
    io::Write,
};

pub(crate) struct JpegTile<'a> {
    pub bytes: &'a [u8],
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

pub(crate) enum StitchOutcome {
    Stitched(StitchStats),
    Unsupported(String),
}

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
pub(crate) struct StitchStats {
    read_ms: f64,
    copy_ms: f64,
    write_ms: f64,
    coefficient_bytes: u64,
}
unsafe extern "C" {
    fn oxy_libjpeg_stitch_coefficients(
        tiles: *const InputTile,
        count: usize,
        width: u32,
        height: u32,
        memory_budget: u64,
        context: *mut c_void,
        cancelled: extern "C" fn(*mut c_void) -> i32,
        write: extern "C" fn(*mut c_void, *const u8, usize) -> i32,
        stats: *mut StitchStats,
        message: *mut c_char,
        message_size: usize,
    ) -> i32;
}

struct Context<'a> {
    output: &'a mut dyn Write,
    cancelled: &'a dyn Fn() -> bool,
    error: Option<std::io::Error>,
}
extern "C" fn cancelled(context: *mut c_void) -> i32 {
    // SAFETY: the synchronous call retains this uniquely borrowed context.
    let context = unsafe { &*(context.cast::<Context<'_>>()) };
    i32::from(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(context.cancelled)).unwrap_or(true),
    )
}
extern "C" fn write(context: *mut c_void, bytes: *const u8, length: usize) -> i32 {
    // SAFETY: C supplies a live output buffer and exclusive context synchronously.
    let context = unsafe { &mut *(context.cast::<Context<'_>>()) };
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        context.output.write_all(bytes)
    }))
    .unwrap_or_else(|_| Err(std::io::Error::other("JPEG output callback panicked")));
    match result {
        Ok(()) => 0,
        Err(error) => {
            context.error = Some(error);
            1
        }
    }
}

pub(crate) fn stitch_coefficients(
    dimensions: PixelDimensions,
    tiles: &[JpegTile<'_>],
    memory_budget: u64,
    output: &mut dyn Write,
    cancel: &dyn Fn() -> bool,
) -> Result<StitchOutcome, MediaError> {
    if cancel() {
        return Err(MediaError::Cancelled);
    }
    let inputs: Vec<_> = tiles
        .iter()
        .map(|tile| InputTile {
            data: tile.bytes.as_ptr(),
            length: tile.bytes.len(),
            x: tile.x,
            y: tile.y,
            width: tile.width,
            height: tile.height,
        })
        .collect();
    let mut context = Context {
        output,
        cancelled: cancel,
        error: None,
    };
    let mut stats = StitchStats::default();
    let mut message = [0 as c_char; 256];
    // SAFETY: borrowed tiles and callbacks outlive the synchronous C call.
    // Callbacks contain unwinds, and libjpeg longjmps remain inside C.
    let status = unsafe {
        oxy_libjpeg_stitch_coefficients(
            inputs.as_ptr(),
            inputs.len(),
            dimensions.width,
            dimensions.height,
            memory_budget,
            (&mut context as *mut Context<'_>).cast(),
            cancelled,
            write,
            &mut stats,
            message.as_mut_ptr(),
            message.len(),
        )
    };
    // SAFETY: zero-initialized buffer is NUL-terminated by the C adapter.
    let reason = unsafe { CStr::from_ptr(message.as_ptr()) }.to_string_lossy();
    match status {
        0 => Ok(StitchOutcome::Stitched(stats)),
        1 => Ok(StitchOutcome::Unsupported(reason.into_owned())),
        2 => Err(MediaError::Cancelled),
        _ => Err(context.error.map_or_else(
            || MediaError::CacheArtifact(format!("JPEG stitch failed: {reason}")),
            MediaError::from,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct FailingWriter;
    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("fixture write failure"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn cancellation_budget_corruption_and_io_preserve_the_safe_boundary() {
        let pixels = image::RgbImage::from_fn(256, 256, |x, y| {
            image::Rgb([
                (x * 13 % 256) as u8,
                (y * 17 % 256) as u8,
                ((x + y) * 7 % 256) as u8,
            ])
        });
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
            .encode_image(&image::DynamicImage::ImageRgb8(pixels))
            .unwrap();
        let dimensions = PixelDimensions {
            width: 256,
            height: 256,
        };
        let tile = |bytes| JpegTile {
            bytes,
            x: 0,
            y: 0,
            width: 256,
            height: 256,
        };
        let polls = Cell::new(0);
        let mut output = Vec::new();
        assert!(matches!(
            stitch_coefficients(
                dimensions,
                &[tile(&bytes)],
                256 * 1024 * 1024,
                &mut output,
                &|| {
                    polls.set(polls.get() + 1);
                    false
                }
            )
            .unwrap(),
            StitchOutcome::Stitched(_)
        ));
        let complete_polls = polls.get();
        for point in [1, 10, complete_polls / 2, complete_polls - 1] {
            polls.set(0);
            assert!(matches!(
                stitch_coefficients(
                    dimensions,
                    &[tile(&bytes)],
                    256 * 1024 * 1024,
                    &mut Vec::new(),
                    &|| {
                        polls.set(polls.get() + 1);
                        polls.get() >= point
                    }
                ),
                Err(MediaError::Cancelled)
            ));
        }
        assert!(matches!(
            stitch_coefficients(
                dimensions,
                &[tile(&bytes)],
                256 * 1024 * 1024,
                &mut FailingWriter,
                &|| false
            ),
            Err(MediaError::Io(_))
        ));
        assert!(matches!(
            stitch_coefficients(dimensions, &[tile(&bytes)], 1, &mut Vec::new(), &|| false)
                .unwrap(),
            StitchOutcome::Unsupported(_)
        ));
        assert!(matches!(
            stitch_coefficients(
                dimensions,
                &[tile(&bytes[..bytes.len() / 2])],
                256 * 1024 * 1024,
                &mut Vec::new(),
                &|| false
            ),
            Err(MediaError::CacheArtifact(_))
        ));
    }
}
