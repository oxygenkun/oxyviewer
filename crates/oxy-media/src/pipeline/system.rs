#[cfg(target_os = "macos")]
use crate::backends::apple_image_io;
use crate::{
    MediaError,
    cache::{cache_tempfile, persist_atomically, preview_cache_key},
    media_source::{cached_preview_result, has_complete_jpeg_markers, preview_result},
    policy::SYSTEM_PREVIEW,
};
use oxy_domain::{PreviewKind, PreviewResult, RenderLevel};
use std::path::Path;

pub fn preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    level: RenderLevel,
) -> Result<PreviewResult, MediaError> {
    std::fs::create_dir_all(cache_dir)?;
    let destination = cache_dir.join(format!(
        "{}.jpg",
        preview_cache_key(path, SYSTEM_PREVIEW, max_size)?
    ));
    if let Some(result) = cached_preview_result(destination.clone(), PreviewKind::System, level)? {
        return Ok(result);
    }
    let temporary = cache_tempfile(&destination, ".jpg")?;
    generate(path, temporary.path(), max_size)?;
    if !has_complete_jpeg_markers(temporary.path())? {
        return Err(MediaError::PreviewGenerationFailed {
            path: path.to_owned(),
            message: "native preview produced a truncated JPEG".into(),
        });
    }
    persist_atomically(temporary, &destination)?;
    preview_result(destination, PreviewKind::System, level)
}

#[cfg(target_os = "macos")]
fn generate(source: &Path, destination: &Path, max_size: u32) -> Result<(), MediaError> {
    apple_image_io::render_jpeg(source, destination, Some(max_size), 90)
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn generate(_source: &Path, _destination: &Path, _max_size: u32) -> Result<(), MediaError> {
    Err(MediaError::NativeDecoderUnavailable)
}
