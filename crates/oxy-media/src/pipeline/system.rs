#[cfg(target_os = "macos")]
use crate::backends::apple_image_io;
use crate::{
    MediaError,
    cache::{persist_atomically, preview_cache_key},
    media_source::{has_complete_jpeg_markers, preview_result},
};
use oxy_domain::{PreviewKind, PreviewResult};
use std::path::Path;

const SYSTEM_CACHE_VERSION: &str = "apple-image-io-preview-v3-jpeg";

pub fn preview(path: &Path, cache_dir: &Path, max_size: u32) -> Result<PreviewResult, MediaError> {
    std::fs::create_dir_all(cache_dir)?;
    let destination = cache_dir.join(format!(
        "{}.jpg",
        preview_cache_key(path, SYSTEM_CACHE_VERSION, max_size)?
    ));
    if destination.is_file() {
        return preview_result(destination, PreviewKind::System);
    }
    let temporary = tempfile::Builder::new()
        .suffix(".jpg")
        .tempfile_in(cache_dir)?;
    generate(path, temporary.path(), max_size)?;
    if !has_complete_jpeg_markers(temporary.path())? {
        return Err(MediaError::PreviewGenerationFailed {
            path: path.to_owned(),
            message: "native preview produced a truncated JPEG".into(),
        });
    }
    persist_atomically(temporary, &destination)?;
    preview_result(destination, PreviewKind::System)
}

#[cfg(target_os = "macos")]
fn generate(source: &Path, destination: &Path, max_size: u32) -> Result<(), MediaError> {
    apple_image_io::render_jpeg(source, destination, Some(max_size), 90)
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn generate(_source: &Path, _destination: &Path, _max_size: u32) -> Result<(), MediaError> {
    Err(MediaError::NativeDecoderUnavailable)
}
