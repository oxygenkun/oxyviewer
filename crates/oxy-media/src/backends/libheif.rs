//! Portable libheif adapter used by preview and tile-session fallbacks.

use image::DynamicImage;
use libheif_rs::{ColorSpace, DecodingOptions, HeifContext, LibHeif, RgbChroma};
use std::path::Path;

use crate::{ImageDimensions, MediaError};

pub fn dimensions(path: &Path) -> Result<ImageDimensions, MediaError> {
    let context = open(path)?;
    let handle = context
        .primary_image_handle()
        .map_err(|error| heif_error(path, error))?;
    Ok(ImageDimensions {
        width: handle.width(),
        height: handle.height(),
    })
}

pub fn decode_scaled(path: &Path, max_size: u32) -> Result<DynamicImage, MediaError> {
    let context = open(path)?;
    let handle = context
        .primary_image_handle()
        .map_err(|error| heif_error(path, error))?;
    let original_longest = handle.width().max(handle.height());
    // Like nomacs, prefer a container thumbnail before decoding the primary
    // image. Use the smallest thumbnail that satisfies the request, or the
    // largest available thumbnail as a fast progressive first stage.
    if max_size <= 2048 {
        let mut ids = Vec::new();
        handle.thumbnail_ids(&mut ids);
        let mut thumbnails = ids
            .into_iter()
            .filter_map(|id| handle.thumbnail(id).ok())
            .collect::<Vec<_>>();
        thumbnails.sort_by_key(|thumbnail| thumbnail.width().max(thumbnail.height()));
        if let Some(thumbnail) = thumbnails
            .iter()
            .find(|thumbnail| thumbnail.width().max(thumbnail.height()) >= max_size)
            .or_else(|| thumbnails.last())
        {
            if let Ok(image) =
                decode_preview_handle(thumbnail, path, Some(preview_thread_limit(max_size)))
            {
                return Ok(image.thumbnail(max_size, max_size));
            }
        }
    }

    // Decode the primary image to display-oriented 8-bit RGB, then scale in
    // libheif before copying pixels into the cache image. Full-detail requests
    // retain the separate high-bit-depth, color-managed path above.
    // This avoids unpacking and color-converting millions of pixels that will be
    // discarded by the subsequent resize.
    let bit_depth = handle.luma_bits_per_pixel();
    let mut options = decoding_options(Some(preview_thread_limit(max_size)));
    if let Some(ref mut opts) = options {
        if bit_depth > 8 {
            opts.set_convert_hdr_to_8bit(true);
        }
    }
    let image = LibHeif::new()
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), options)
        .map_err(|error| heif_error(path, error))?;

    let scale = (max_size as f64 / original_longest as f64).min(1.0);
    let target_w = ((handle.width() as f64 * scale).round() as u32).max(1);
    let target_h = ((handle.height() as f64 * scale).round() as u32).max(1);
    let scaled = image
        .scale(target_w, target_h, None)
        .map_err(|error| heif_error(path, error))?;

    let plane = scaled
        .planes()
        .interleaved
        .ok_or_else(|| MediaError::Heif {
            path: path.to_owned(),
            message: "decoded image has no interleaved RGB plane".into(),
        })?;
    image_from_rgb8_plane(plane.data, plane.width, plane.height, plane.stride)
}

pub fn decode_full_rgb8(path: &Path) -> Result<DynamicImage, MediaError> {
    let context = open(path)?;
    let handle = context
        .primary_image_handle()
        .map_err(|error| heif_error(path, error))?;
    decode_preview_handle(&handle, path, None)
}

fn decode_preview_handle(
    handle: &libheif_rs::ImageHandle,
    path: &Path,
    thread_limit: Option<u32>,
) -> Result<DynamicImage, MediaError> {
    let bit_depth = handle.luma_bits_per_pixel();
    let mut options = decoding_options(thread_limit);
    if let Some(ref mut opts) = options {
        if bit_depth > 8 {
            opts.set_convert_hdr_to_8bit(true);
        }
    }
    let image = LibHeif::new()
        .decode(handle, ColorSpace::Rgb(RgbChroma::Rgb), options)
        .map_err(|error| heif_error(path, error))?;
    let plane = image.planes().interleaved.ok_or_else(|| MediaError::Heif {
        path: path.to_owned(),
        message: "decoded image has no interleaved RGB plane".into(),
    })?;
    image_from_rgb8_plane(plane.data, plane.width, plane.height, plane.stride)
}

fn image_from_rgb8_plane(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<DynamicImage, MediaError> {
    let pixels = unpack_rgb_8_raw(data, width, height, stride)?;
    let buffer = image::ImageBuffer::<image::Rgb<u8>, Vec<u8>>::from_vec(width, height, pixels)
        .expect("pixel buffer dimensions match image dimensions");
    Ok(DynamicImage::ImageRgb8(buffer))
}

fn open(path: &Path) -> Result<HeifContext<'static>, MediaError> {
    let path_string = path.to_string_lossy();
    HeifContext::read_from_file(&path_string).map_err(|error| heif_error(path, error))
}

fn heif_error(path: &Path, error: impl std::fmt::Display) -> MediaError {
    MediaError::Heif {
        path: path.to_owned(),
        message: error.to_string(),
    }
}

/// Build [`DecodingOptions`] with an optional workload-specific thread limit.
fn decoding_options(thread_limit: Option<u32>) -> Option<DecodingOptions> {
    let mut options = DecodingOptions::new()?;
    let available = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    let threads = thread_limit.unwrap_or(available).min(available).max(1);
    options.set_num_codec_threads(threads);
    options.set_num_library_threads(threads);
    Some(options)
}

fn preview_thread_limit(max_size: u32) -> u32 {
    match max_size {
        0..=512 => 1,
        513..=2_048 => 2,
        _ => 4,
    }
}

fn unpack_rgb_8_raw(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<Vec<u8>, MediaError> {
    let row_bytes = width as usize * 3;
    if data.len() < stride * height as usize {
        return Err(MediaError::Color("invalid decoded HEIF RGB stride".into()));
    }
    let mut pixels = Vec::with_capacity(row_bytes * height as usize);
    for row in data.chunks_exact(stride).take(height as usize) {
        pixels.extend_from_slice(&row[..row_bytes]);
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn linked_libheif_meets_security_baseline() {
        let version = LibHeif::new().version();
        assert!(
            version >= [1, 23, 3],
            "linked libheif {version:?} is older than the required 1.23.3"
        );
    }

    #[test]
    fn preview_thread_limits_keep_grid_decode_lightweight() {
        assert_eq!(preview_thread_limit(512), 1);
        assert_eq!(preview_thread_limit(1_024), 2);
        assert_eq!(preview_thread_limit(4_096), 4);
    }
}
