use crate::MediaError;
use std::{os::unix::ffi::OsStrExt, path::Path};

unsafe extern "C" {
    fn oxy_apple_core_image_render_raw_jpeg(
        source_path: *const u8,
        source_path_len: usize,
        destination_path: *const u8,
        destination_path_len: usize,
        max_size: u32,
        quality: u8,
    ) -> i32;
}

pub fn render_raw_jpeg(
    source: &Path,
    destination: &Path,
    max_size: Option<u32>,
    quality: u8,
) -> Result<(), MediaError> {
    let source = source.as_os_str().as_bytes();
    let destination = destination.as_os_str().as_bytes();
    let status = unsafe {
        oxy_apple_core_image_render_raw_jpeg(
            source.as_ptr(),
            source.len(),
            destination.as_ptr(),
            destination.len(),
            max_size.unwrap_or(0),
            quality,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(MediaError::NativeDecode {
            backend: "Apple Core Image RAW",
            message: format!("render failed at native stage {status}"),
        })
    }
}
