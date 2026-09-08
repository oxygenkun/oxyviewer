use super::artifact::PREVIEW_CACHE_SIZES;
#[cfg(target_os = "macos")]
use crate::backends::{apple_core_image, apple_image_io};
use crate::{
    ImageDimensions, MediaError,
    backends::libraw,
    cache::{
        cache_tempfile, persist_atomically, preview_cache_key, write_bytes_atomically_cancelled,
        write_jpeg_atomically, write_jpeg_atomically_cancelled,
    },
    decode_control::{
        DecodePriority, acquire_decode, acquire_file_lock, acquire_raw_full_decode, file_lock,
    },
    media_source::{cached_preview_result, has_complete_jpeg_markers, preview_result},
    policy::{RAW_FULL, RAW_PREVIEW},
    presentation::{CAMERA_JPEG, RAW_DEVELOPED_JPEG, camera_preview_can_satisfy_raw_full},
};
use oxy_domain::{PreviewKind, PreviewResult, RenderLevel};
use oxy_runtime::CancellationToken;
use std::path::Path;

const RAW_DEVELOPED_SUFFIXES: [&str; 3] =
    ["core-image.jpg", "image-io.jpg", "libraw-developed.jpg"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawBackend {
    AppleCoreImage,
    AppleImageIo,
    LibRawDevelopment,
}

pub(crate) fn backend_plan(level: RenderLevel) -> Vec<RawBackend> {
    #[cfg(target_os = "macos")]
    return match level {
        RenderLevel::Thumbnail => vec![
            RawBackend::AppleImageIo,
            RawBackend::AppleCoreImage,
            RawBackend::LibRawDevelopment,
        ],
        RenderLevel::Preview | RenderLevel::Full => vec![
            RawBackend::AppleCoreImage,
            RawBackend::AppleImageIo,
            RawBackend::LibRawDevelopment,
        ],
    };
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        let _ = level;
        vec![RawBackend::LibRawDevelopment]
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
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let max_size = max_size.max(1);
    let level = if max_size <= 512 {
        RenderLevel::Thumbnail
    } else {
        RenderLevel::Preview
    };
    std::fs::create_dir_all(cache_dir)?;
    let cache_key = preview_cache_key(path, RAW_PREVIEW, max_size)?;
    if let Some(result) = cached_result(cache_dir, &cache_key, level)? {
        return Ok(result);
    }
    if let Some(result) = larger_cached_preview(path, cache_dir, max_size, level)? {
        return Ok(result);
    }

    let _decode_permit = acquire_decode(priority, &|| cancellation.is_cancelled())?;
    let source_lock_key = preview_cache_key(path, "raw-source-decode", 0)?;
    let source_lock = file_lock(&source_lock_key);
    let _decode_guard = acquire_file_lock(&source_lock, &|| cancellation.is_cancelled())?;
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    if let Some(result) = cached_result(cache_dir, &cache_key, level)? {
        return Ok(result);
    }
    if let Some(result) = larger_cached_preview(path, cache_dir, max_size, level)? {
        return Ok(result);
    }

    let result = match cache_embedded(path, cache_dir, &cache_key, max_size, level, cancellation) {
        Ok(result) => Ok(result),
        Err(embedded_error) => render_developed(
            path,
            cache_dir,
            &cache_key,
            Some(max_size),
            level,
            Some(embedded_error.to_string()),
            cancellation,
        ),
    };
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    result
}

pub(crate) fn full(
    path: &Path,
    cache_dir: &Path,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    std::fs::create_dir_all(cache_dir)?;
    let source_size = dimensions(path)?;
    let preview_key = preview_cache_key(path, RAW_PREVIEW, 4_096)?;
    let embedded = cached_embedded(cache_dir, &preview_key, RenderLevel::Full)?.or_else(|| {
        cache_embedded(
            path,
            cache_dir,
            &preview_key,
            4_096,
            RenderLevel::Full,
            cancellation,
        )
        .ok()
    });
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

    let cache_key = preview_cache_key(path, RAW_FULL, 0)?;
    if let Some(result) = cached_developed(cache_dir, &cache_key, RenderLevel::Full)? {
        return Ok(result);
    }
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let _decode_guard = acquire_raw_full_decode(&|| cancellation.is_cancelled())?;
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    if let Some(result) = cached_developed(cache_dir, &cache_key, RenderLevel::Full)? {
        return Ok(result);
    }
    render_developed(
        path,
        cache_dir,
        &cache_key,
        None,
        RenderLevel::Full,
        None,
        cancellation,
    )
}

fn cached_embedded(
    cache_dir: &Path,
    cache_key: &str,
    level: RenderLevel,
) -> Result<Option<PreviewResult>, MediaError> {
    let path = cache_dir.join(format!("{cache_key}.embedded.jpg"));
    cached_preview_result(path, PreviewKind::Embedded, level)
}

fn cached_developed(
    cache_dir: &Path,
    cache_key: &str,
    level: RenderLevel,
) -> Result<Option<PreviewResult>, MediaError> {
    for suffix in RAW_DEVELOPED_SUFFIXES {
        let path = cache_dir.join(format!("{cache_key}.{suffix}"));
        if let Some(result) = cached_preview_result(path, PreviewKind::Developed, level)? {
            return Ok(Some(result));
        }
    }
    Ok(None)
}

fn cached_result(
    cache_dir: &Path,
    cache_key: &str,
    level: RenderLevel,
) -> Result<Option<PreviewResult>, MediaError> {
    if let Some(result) = cached_embedded(cache_dir, cache_key, level)? {
        return Ok(Some(result));
    }
    cached_developed(cache_dir, cache_key, level)
}

fn cache_embedded(
    path: &Path,
    cache_dir: &Path,
    cache_key: &str,
    max_size: u32,
    level: RenderLevel,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let destination = cache_dir.join(format!("{cache_key}.embedded.jpg"));
    match libraw::embedded(path, max_size).map_err(|message| MediaError::LibRaw {
        path: path.to_owned(),
        message,
    })? {
        libraw::Preview::EmbeddedJpeg(data) => {
            write_bytes_atomically_cancelled(&data, &destination, || cancellation.is_cancelled())?;
        }
        libraw::Preview::EmbeddedImage(image) => {
            write_jpeg_atomically_cancelled(&image, &destination, 90, CAMERA_JPEG, || {
                cancellation.is_cancelled()
            })?;
        }
    }
    preview_result(destination, PreviewKind::Embedded, level)
}

fn render_developed(
    path: &Path,
    cache_dir: &Path,
    cache_key: &str,
    max_size: Option<u32>,
    level: RenderLevel,
    initial_error: Option<String>,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    let quality = if level == RenderLevel::Full { 95 } else { 90 };
    let mut errors = initial_error.into_iter().collect::<Vec<_>>();
    for backend in backend_plan(level) {
        let suffix = match backend {
            RawBackend::AppleCoreImage => "core-image.jpg",
            RawBackend::AppleImageIo => "image-io.jpg",
            RawBackend::LibRawDevelopment => "libraw-developed.jpg",
        };
        let destination = cache_dir.join(format!("{cache_key}.{suffix}"));
        if let Some(result) =
            cached_preview_result(destination.clone(), PreviewKind::Developed, level)?
        {
            return Ok(result);
        }
        let temporary = cache_tempfile(&destination, ".jpg")?;
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
                if cancellation.is_cancelled() {
                    return Err(MediaError::Cancelled);
                }
                persist_atomically(temporary, &destination)?;
                return preview_result(destination, PreviewKind::Developed, level);
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
    level: RenderLevel,
) -> Result<Option<PreviewResult>, MediaError> {
    for candidate_size in PREVIEW_CACHE_SIZES
        .iter()
        .copied()
        .filter(|&size| size > max_size)
    {
        let key = preview_cache_key(path, RAW_PREVIEW, candidate_size)?;
        for (suffix, kind) in std::iter::once(("embedded.jpg", PreviewKind::Embedded)).chain(
            RAW_DEVELOPED_SUFFIXES
                .into_iter()
                .map(|suffix| (suffix, PreviewKind::Developed)),
        ) {
            let candidate = cache_dir.join(format!("{key}.{suffix}"));
            if let Some(result) = cached_preview_result(candidate, kind, level)? {
                return Ok(Some(result));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_thumbnail_prefers_measured_image_io_path() {
        assert_eq!(
            backend_plan(RenderLevel::Thumbnail),
            vec![
                RawBackend::AppleImageIo,
                RawBackend::AppleCoreImage,
                RawBackend::LibRawDevelopment,
            ]
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_detail_prefers_measured_core_image_path() {
        for level in [RenderLevel::Preview, RenderLevel::Full] {
            assert_eq!(
                backend_plan(level),
                vec![
                    RawBackend::AppleCoreImage,
                    RawBackend::AppleImageIo,
                    RawBackend::LibRawDevelopment,
                ]
            );
        }
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn portable_platforms_use_libraw_development() {
        for level in [
            RenderLevel::Thumbnail,
            RenderLevel::Preview,
            RenderLevel::Full,
        ] {
            assert_eq!(backend_plan(level), vec![RawBackend::LibRawDevelopment]);
        }
    }
}
