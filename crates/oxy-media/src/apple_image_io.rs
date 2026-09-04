use crate::MediaError;
use image::{DynamicImage, ImageBuffer, Rgba, RgbaImage};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::unix::ffi::OsStrExt,
    path::Path,
    ptr, slice,
    sync::Mutex,
};

// ImageIO can report a successful JPEG finalize while concurrent operations on
// the same large HEIF source leave the destination truncated. Serialize full
// transcodes, but keep thumbnail decode independent so a background cache
// write cannot block the selected image.
static HEIF_TRANSCODE_LOCK: Mutex<()> = Mutex::new(());

fn lock_heif_transcode() -> std::sync::MutexGuard<'static, ()> {
    HEIF_TRANSCODE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

unsafe extern "C" {
    fn oxy_apple_image_io_can_decode(path: *const u8, path_len: usize) -> i32;
    fn oxy_apple_image_io_decode_rgba8(
        path: *const u8,
        path_len: usize,
        max_size: u32,
        pixels: *mut *mut u8,
        pixels_len: *mut usize,
        width: *mut u32,
        height: *mut u32,
    ) -> i32;
    fn oxy_apple_image_io_write_jpeg(
        path: *const u8,
        path_len: usize,
        pixels: *const u8,
        pixels_len: usize,
        width: u32,
        height: u32,
        quality: u8,
    ) -> i32;
    fn oxy_apple_image_io_transcode_jpeg(
        source_path: *const u8,
        source_path_len: usize,
        destination_path: *const u8,
        destination_path_len: usize,
        quality: u8,
    ) -> i32;
    fn oxy_apple_image_io_sharpen_rgba8(
        pixels: *mut u8,
        pixels_len: usize,
        width: u32,
        height: u32,
    ) -> i32;
    fn oxy_apple_image_io_free(pixels: *mut u8);
}

pub fn capability() -> Result<(), MediaError> {
    Ok(())
}

pub fn can_decode(path: &Path) -> Result<(), MediaError> {
    let path = path.as_os_str().as_bytes();
    if unsafe { oxy_apple_image_io_can_decode(path.as_ptr(), path.len()) } != 0 {
        Ok(())
    } else {
        Err(native_error("ImageIO could not open the selected image"))
    }
}

pub fn decode_rgba8(path: &Path, max_size: u32) -> Result<DynamicImage, MediaError> {
    let path_bytes = path.as_os_str().as_bytes();
    let mut pixels = ptr::null_mut();
    let mut pixels_len = 0;
    let mut width = 0;
    let mut height = 0;
    let status = unsafe {
        oxy_apple_image_io_decode_rgba8(
            path_bytes.as_ptr(),
            path_bytes.len(),
            max_size.max(1),
            &mut pixels,
            &mut pixels_len,
            &mut width,
            &mut height,
        )
    };
    if status != 0 {
        return Err(native_error(format!(
            "decode failed at native stage {status}"
        )));
    }
    let expected_len = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .and_then(|stride| {
            usize::try_from(height)
                .ok()
                .and_then(|height| stride.checked_mul(height))
        })
        .ok_or_else(|| native_error("decoded image buffer size overflow"))?;
    if pixels.is_null() || pixels_len != expected_len {
        if !pixels.is_null() {
            unsafe { oxy_apple_image_io_free(pixels) };
        }
        return Err(native_error("decoded RGBA length mismatch"));
    }
    let data = unsafe { slice::from_raw_parts(pixels, pixels_len).to_vec() };
    unsafe { oxy_apple_image_io_free(pixels) };
    let image = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_vec(width, height, data)
        .ok_or_else(|| native_error("decoded image buffer dimensions do not match"))?;
    Ok(DynamicImage::ImageRgba8(image))
}

pub fn write_jpeg(image: &DynamicImage, path: &Path, quality: u8) -> Result<(), MediaError> {
    let converted;
    let rgba = if let Some(rgba) = image.as_rgba8() {
        rgba
    } else {
        converted = image.to_rgba8();
        &converted
    };
    let path_bytes = path.as_os_str().as_bytes();
    let status = unsafe {
        oxy_apple_image_io_write_jpeg(
            path_bytes.as_ptr(),
            path_bytes.len(),
            rgba.as_raw().as_ptr(),
            rgba.as_raw().len(),
            rgba.width(),
            rgba.height(),
            quality,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(native_error(format!(
            "JPEG encode failed at native stage {status}"
        )))
    }
}

pub fn transcode_jpeg(source: &Path, destination: &Path, quality: u8) -> Result<(), MediaError> {
    let guard = lock_heif_transcode();
    let source_bytes = source.as_os_str().as_bytes();
    let destination_bytes = destination.as_os_str().as_bytes();
    let status = unsafe {
        oxy_apple_image_io_transcode_jpeg(
            source_bytes.as_ptr(),
            source_bytes.len(),
            destination_bytes.as_ptr(),
            destination_bytes.len(),
            quality,
        )
    };
    let native_failure = if status != 0 {
        format!("native stage {status}")
    } else {
        match has_complete_jpeg_markers(destination) {
            Ok(true) => return Ok(()),
            Ok(false) => "native finalize produced a truncated JPEG".into(),
            Err(error) => format!("could not validate JPEG output: {error}"),
        }
    };

    // A failed ImageIO finalize is repeatable while the system decoder is
    // under pressure. Release its lane and use FFmpeg's independent decoder
    // instead of publishing the small header-only file as a valid cache hit.
    drop(guard);
    OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(destination)?;
    let dimensions = crate::heif::dimensions(source)?;
    if let Err(fallback_error) =
        crate::ffmpeg_heif::transcode_full_jpeg(source, destination, dimensions, quality)
    {
        return Err(native_error(format!(
            "source HEIF to JPEG conversion failed ({native_failure}); FFmpeg fallback failed: {fallback_error}"
        )));
    }
    if has_complete_jpeg_markers(destination)? {
        Ok(())
    } else {
        Err(native_error(format!(
            "source HEIF to JPEG conversion failed ({native_failure}); FFmpeg fallback produced a truncated JPEG"
        )))
    }
}

fn has_complete_jpeg_markers(path: &Path) -> Result<bool, std::io::Error> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() < 4 {
        return Ok(false);
    }
    let mut start = [0_u8; 2];
    file.read_exact(&mut start)?;
    file.seek(SeekFrom::End(-2))?;
    let mut end = [0_u8; 2];
    file.read_exact(&mut end)?;
    Ok(start == [0xff, 0xd8] && end == [0xff, 0xd9])
}

pub fn sharpen_rgba8(image: &mut RgbaImage) -> Result<(), MediaError> {
    let width = image.width();
    let height = image.height();
    let pixels = image.as_mut();
    let status = unsafe {
        oxy_apple_image_io_sharpen_rgba8(pixels.as_mut_ptr(), pixels.len(), width, height)
    };
    if status == 0 {
        Ok(())
    } else {
        Err(native_error(format!(
            "display sharpening failed at native stage {status}"
        )))
    }
}

fn native_error(message: impl Into<String>) -> MediaError {
    MediaError::NativeDecode {
        backend: "Apple ImageIO",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_repository_heif_fixture() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        if let Err(error) = can_decode(&fixture) {
            eprintln!("skipping unsupported ImageIO HEIF fixture: {error}");
            return;
        }
        let image = decode_rgba8(&fixture, 512).unwrap();
        assert!((510..=512).contains(&image.width().max(image.height())));
    }

    #[test]
    fn writes_jpeg_with_image_io() {
        let output = tempfile::Builder::new().suffix(".jpg").tempfile().unwrap();
        let image = DynamicImage::new_rgba8(32, 16);
        write_jpeg(&image, output.path(), 90).unwrap();
        let decoded = image::ImageReader::open(output.path())
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((decoded.width(), decoded.height()), (32, 16));
    }

    #[test]
    fn transcodes_repository_heif_directly_to_jpeg() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/DSC00449.HIF");
        if can_decode(&fixture).is_err() {
            return;
        }
        let output = tempfile::Builder::new().suffix(".jpg").tempfile().unwrap();
        transcode_jpeg(&fixture, output.path(), 95).unwrap();
        let decoded = image::ImageReader::open(output.path())
            .unwrap()
            .decode()
            .unwrap();
        let mut edges = [decoded.width(), decoded.height()];
        edges.sort_unstable();
        assert_eq!(edges, [4672, 7008]);
    }

    #[test]
    fn sharpens_rgba_with_accelerate() {
        let mut image = RgbaImage::from_pixel(3, 3, Rgba([64, 64, 64, 255]));
        image.put_pixel(1, 1, Rgba([128, 128, 128, 255]));
        sharpen_rgba8(&mut image).unwrap();
        assert!(image.get_pixel(1, 1)[0] > 128);
        assert_eq!(image.get_pixel(1, 1)[3], 255);
    }
}
