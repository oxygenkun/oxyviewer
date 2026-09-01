use crate::MediaError;
use image::{DynamicImage, ImageBuffer, Rgba};
use std::{os::unix::ffi::OsStrExt, path::Path, ptr, slice};

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
}
