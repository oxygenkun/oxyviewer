use super::{artifact::PREVIEW_CACHE_SIZES, planner::Platform};
#[cfg(target_os = "macos")]
use crate::backends::{apple_core_image, apple_image_io};
use crate::{
    DecodePriority, ImageDimensions, MediaError,
    backends::libraw,
    cache::{persist_atomically, preview_cache_key, write_bytes_atomically, write_jpeg_atomically},
    decode_control::{acquire_decode, acquire_file_lock, acquire_raw_full_decode},
    media_source::{has_complete_jpeg_markers, preview_result},
    presentation::{CAMERA_JPEG, RAW_DEVELOPED_JPEG, camera_preview_can_satisfy_raw_full},
};
use oxy_domain::{PreviewKind, PreviewResult, RenderLevel};
use std::path::Path;

const LIBRAW_CACHE_VERSION: &str = "raw-native-v8-camera-or-developed-srgb";
const LIBRAW_FULL_CACHE_VERSION: &str = "raw-native-full-v4-srgb";
const RAW_DEVELOPED_SUFFIXES: [&str; 3] =
    ["core-image.jpg", "image-io.jpg", "libraw-developed.jpg"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawBackend {
    AppleCoreImage,
    AppleImageIo,
    LibRawDevelopment,
}

pub(crate) fn backend_plan(platform: Platform, level: RenderLevel) -> Vec<RawBackend> {
    match (platform, level) {
        (Platform::Macos, RenderLevel::Thumbnail) => vec![
            RawBackend::AppleImageIo,
            RawBackend::AppleCoreImage,
            RawBackend::LibRawDevelopment,
        ],
        (Platform::Macos, RenderLevel::Preview | RenderLevel::Full) => vec![
            RawBackend::AppleCoreImage,
            RawBackend::AppleImageIo,
            RawBackend::LibRawDevelopment,
        ],
        (Platform::Windows | Platform::Linux, _) => vec![RawBackend::LibRawDevelopment],
    }
}

pub(crate) fn dimensions(path: &Path) -> Result<ImageDimensions, MediaError> {
    libraw::dimensions(path).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    })
}

pub(crate) fn preview_with_priority(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
    priority: DecodePriority,
) -> Result<PreviewResult, MediaError> {
    let max_size = max_size.max(1);
    std::fs::create_dir_all(cache_dir)?;
    let cache_key = preview_cache_key(path, LIBRAW_CACHE_VERSION, max_size)?;
    if let Some(result) = cached_result(cache_dir, &cache_key)? {
        return Ok(result);
    }
    if let Some(result) = larger_cached_preview(path, cache_dir, max_size)? {
        return Ok(result);
    }

    let _decode_permit = acquire_decode(priority);
    let source_lock_key = preview_cache_key(path, "raw-source-decode", 0)?;
    let (_lock_arc, _decode_guard) = acquire_file_lock(&source_lock_key);
    if let Some(result) = cached_result(cache_dir, &cache_key)? {
        return Ok(result);
    }
    if let Some(result) = larger_cached_preview(path, cache_dir, max_size)? {
        return Ok(result);
    }

    match cache_embedded(path, cache_dir, &cache_key, max_size) {
        Ok(result) => Ok(result),
        Err(embedded_error) => {
            let level = if max_size <= 512 {
                RenderLevel::Thumbnail
            } else {
                RenderLevel::Preview
            };
            render_developed(
                path,
                cache_dir,
                &cache_key,
                Some(max_size),
                level,
                90,
                Some(embedded_error.to_string()),
            )
        }
    }
}

pub(crate) fn full(path: &Path, cache_dir: &Path) -> Result<PreviewResult, MediaError> {
    std::fs::create_dir_all(cache_dir)?;
    let source_size = dimensions(path)?;
    let preview_key = preview_cache_key(path, LIBRAW_CACHE_VERSION, 4_096)?;
    let embedded = cached_embedded(cache_dir, &preview_key)?
        .or_else(|| cache_embedded(path, cache_dir, &preview_key, 4_096).ok());
    if let Some(embedded) = embedded
        && covers_source(
            ImageDimensions {
                width: embedded.width,
                height: embedded.height,
            },
            source_size,
        )
    {
        return Ok(embedded);
    }

    let cache_key = preview_cache_key(path, LIBRAW_FULL_CACHE_VERSION, 0)?;
    if let Some(result) = cached_developed(cache_dir, &cache_key)? {
        return Ok(result);
    }
    let _decode_guard = acquire_raw_full_decode();
    if let Some(result) = cached_developed(cache_dir, &cache_key)? {
        return Ok(result);
    }
    render_developed(
        path,
        cache_dir,
        &cache_key,
        None,
        RenderLevel::Full,
        95,
        None,
    )
}

fn cached_embedded(cache_dir: &Path, cache_key: &str) -> Result<Option<PreviewResult>, MediaError> {
    let path = cache_dir.join(format!("{cache_key}.embedded.jpg"));
    path.is_file()
        .then(|| preview_result(path, PreviewKind::Embedded))
        .transpose()
}

fn cached_developed(
    cache_dir: &Path,
    cache_key: &str,
) -> Result<Option<PreviewResult>, MediaError> {
    for suffix in RAW_DEVELOPED_SUFFIXES {
        let path = cache_dir.join(format!("{cache_key}.{suffix}"));
        if path.is_file() {
            return preview_result(path, PreviewKind::Developed).map(Some);
        }
    }
    Ok(None)
}

fn cached_result(cache_dir: &Path, cache_key: &str) -> Result<Option<PreviewResult>, MediaError> {
    if let Some(result) = cached_embedded(cache_dir, cache_key)? {
        return Ok(Some(result));
    }
    cached_developed(cache_dir, cache_key)
}

fn cache_embedded(
    path: &Path,
    cache_dir: &Path,
    cache_key: &str,
    max_size: u32,
) -> Result<PreviewResult, MediaError> {
    let destination = cache_dir.join(format!("{cache_key}.embedded.jpg"));
    match libraw::embedded(path, max_size).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    })? {
        libraw::Preview::EmbeddedJpeg(data) => write_bytes_atomically(&data, &destination)?,
        libraw::Preview::EmbeddedImage(image) => {
            write_jpeg_atomically(&image, &destination, 90, CAMERA_JPEG)?;
        }
    }
    preview_result(destination, PreviewKind::Embedded)
}

fn render_developed(
    path: &Path,
    cache_dir: &Path,
    cache_key: &str,
    max_size: Option<u32>,
    level: RenderLevel,
    quality: u8,
    initial_error: Option<String>,
) -> Result<PreviewResult, MediaError> {
    let mut errors = initial_error.into_iter().collect::<Vec<_>>();
    for backend in backend_plan(current_platform(), level) {
        let suffix = match backend {
            RawBackend::AppleCoreImage => "core-image.jpg",
            RawBackend::AppleImageIo => "image-io.jpg",
            RawBackend::LibRawDevelopment => "libraw-developed.jpg",
        };
        let destination = cache_dir.join(format!("{cache_key}.{suffix}"));
        if destination.is_file() {
            return preview_result(destination, PreviewKind::Developed);
        }
        let temporary = tempfile::Builder::new()
            .suffix(".jpg")
            .tempfile_in(cache_dir)?;
        let result = match backend {
            #[cfg(target_os = "macos")]
            RawBackend::AppleCoreImage => {
                apple_core_image::render_raw_jpeg(path, temporary.path(), max_size, quality)
            }
            #[cfg(not(target_os = "macos"))]
            RawBackend::AppleCoreImage => Err(MediaError::NativeDecoderUnavailable),
            #[cfg(target_os = "macos")]
            RawBackend::AppleImageIo => {
                apple_image_io::render_jpeg(path, temporary.path(), max_size, quality)
            }
            #[cfg(not(target_os = "macos"))]
            RawBackend::AppleImageIo => Err(MediaError::NativeDecoderUnavailable),
            RawBackend::LibRawDevelopment => libraw::developed(path, max_size)
                .map_err(|message| MediaError::LibRaw {
                    path: path.to_owned(),
                    message,
                })
                .and_then(|image| {
                    let image = if max_size.is_none() {
                        image.unsharpen(0.8, 2)
                    } else {
                        image
                    };
                    write_jpeg_atomically(&image, temporary.path(), quality, RAW_DEVELOPED_JPEG)
                }),
        };
        match result {
            Ok(()) if has_complete_jpeg_markers(temporary.path())? => {
                persist_atomically(temporary, &destination)?;
                return preview_result(destination, PreviewKind::Developed);
            }
            Ok(()) => errors.push(format!("{backend:?}: produced a truncated JPEG")),
            Err(error) => errors.push(format!("{backend:?}: {error}")),
        }
    }
    Err(MediaError::BackendAttempts {
        attempts: errors.join("; "),
        source: Box::new(MediaError::NativeDecoderUnavailable),
    })
}

pub(crate) fn covers_source(candidate: ImageDimensions, source: ImageDimensions) -> bool {
    // Full explicitly permits a near-full camera render for immediate pixel
    // inspection. Geometry alone is insufficient: the representation contract
    // is part of this decision and the result remains `PreviewKind::Embedded`.
    camera_preview_can_satisfy_raw_full(CAMERA_JPEG, candidate, source)
}

fn larger_cached_preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
) -> Result<Option<PreviewResult>, MediaError> {
    for candidate_size in PREVIEW_CACHE_SIZES
        .iter()
        .copied()
        .filter(|&size| size > max_size)
    {
        let key = preview_cache_key(path, LIBRAW_CACHE_VERSION, candidate_size)?;
        for (suffix, kind) in std::iter::once(("embedded.jpg", PreviewKind::Embedded)).chain(
            RAW_DEVELOPED_SUFFIXES
                .into_iter()
                .map(|suffix| (suffix, PreviewKind::Developed)),
        ) {
            let candidate = cache_dir.join(format!("{key}.{suffix}"));
            if candidate.is_file() {
                return preview_result(candidate, kind).map(Some);
            }
        }
    }
    Ok(None)
}

const fn current_platform() -> Platform {
    #[cfg(target_os = "windows")]
    {
        Platform::Windows
    }
    #[cfg(target_os = "macos")]
    {
        Platform::Macos
    }
    #[cfg(target_os = "linux")]
    {
        Platform::Linux
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_thumbnail_prefers_measured_image_io_path() {
        assert_eq!(
            backend_plan(Platform::Macos, RenderLevel::Thumbnail),
            vec![
                RawBackend::AppleImageIo,
                RawBackend::AppleCoreImage,
                RawBackend::LibRawDevelopment,
            ]
        );
    }

    #[test]
    fn macos_detail_prefers_measured_core_image_path() {
        for level in [RenderLevel::Preview, RenderLevel::Full] {
            assert_eq!(
                backend_plan(Platform::Macos, level),
                vec![
                    RawBackend::AppleCoreImage,
                    RawBackend::AppleImageIo,
                    RawBackend::LibRawDevelopment,
                ]
            );
        }
    }

    #[test]
    fn portable_platforms_use_libraw_development() {
        for platform in [Platform::Windows, Platform::Linux] {
            for level in [
                RenderLevel::Thumbnail,
                RenderLevel::Preview,
                RenderLevel::Full,
            ] {
                assert_eq!(
                    backend_plan(platform, level),
                    vec![RawBackend::LibRawDevelopment]
                );
            }
        }
    }
}
