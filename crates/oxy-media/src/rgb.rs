//! Decoded RGB pixels for non-display consumers.
//!
//! The browser and loupe render textures and JPEG resources; the face analyzer
//! needs plain RGB8 bytes. This module is the single decode bridge between the
//! two, so `oxy-faces` never has to know about preview artifacts, formats, or
//! the cache layout.

use std::path::Path;

use oxy_domain::NormalizedRect;

use crate::MediaError;

/// Row-major, top-left origin RGB8 pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbPixels {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbPixels {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self, MediaError> {
        if width == 0 || height == 0 || data.len() != width as usize * height as usize * 3 {
            return Err(MediaError::CacheArtifact(format!(
                "invalid RGB buffer: {width}x{height} with {} bytes",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }
}

/// Decodes a cached preview artifact (the JPEG or PNG that
/// [`crate::PreviewResult::path`] points at) into RGB8.
///
/// Alpha is dropped against white by `to_rgb8`, which matches how a preview is
/// shown on the loupe background and keeps a transparent PNG from turning into
/// a dark face crop.
pub fn decode_rgb_pixels(path: &Path) -> Result<RgbPixels, MediaError> {
    let decoded = image::open(path)?.to_rgb8();
    RgbPixels::new(decoded.width(), decoded.height(), decoded.into_raw())
}

/// Applies the EXIF orientation so pixels are in viewing order.
///
/// The bounded preview path already delivers display-corrected pixels, but
/// `RenderLevel::Full` on a raster asset resolves to the source file itself,
/// whose pixels are stored in encoded order. Analysis coordinates are reported
/// in display orientation (that is what the loupe draws), so a rotated photo
/// must be upright before detection or every box would be wrong.
///
/// An unknown or absent orientation is treated as "already upright" rather than
/// guessed at.
pub fn apply_exif_orientation(
    pixels: RgbPixels,
    exif_orientation: u8,
) -> Result<RgbPixels, MediaError> {
    let Some(orientation) = image::metadata::Orientation::from_exif(exif_orientation) else {
        return Ok(pixels);
    };
    if orientation == image::metadata::Orientation::NoTransforms {
        return Ok(pixels);
    }
    let image =
        image::RgbImage::from_raw(pixels.width, pixels.height, pixels.data).ok_or_else(|| {
            MediaError::CacheArtifact("RGB buffer does not match its dimensions".into())
        })?;
    let mut dynamic = image::DynamicImage::ImageRgb8(image);
    dynamic.apply_orientation(orientation);
    let converted = dynamic.to_rgb8();
    RgbPixels::new(converted.width(), converted.height(), converted.into_raw())
}

/// Crops a square around `region` and encodes it as JPEG.
///
/// This backs the face-crop thumbnails in the people panel. The region is
/// normalized to the source image, so the crop is independent of which render
/// level produced `source`. `margin` expands the square beyond the detected box
/// (1.0 is exactly the box, 1.4 adds hair and chin) and the square is clamped to
/// the image, so a face at the edge is cropped rather than padded.
///
/// Returns `(jpeg_bytes, width, height)`; width and height always equal `size`
/// unless the source is empty, which is an error.
pub fn face_crop_jpeg(
    source: &RgbPixels,
    region: NormalizedRect,
    size: u32,
    margin: f32,
    quality: u8,
) -> Result<(Vec<u8>, u32, u32), MediaError> {
    if size == 0 || source.width == 0 || source.height == 0 {
        return Err(MediaError::CacheArtifact(
            "face crop needs a non-empty image and size".into(),
        ));
    }
    let Some(region) = region.clamp_unit() else {
        return Err(MediaError::CacheArtifact("invalid face region".into()));
    };

    let image_width = source.width as f32;
    let image_height = source.height as f32;
    let center_x = (region.x + region.width / 2.0) * image_width;
    let center_y = (region.y + region.height / 2.0) * image_height;
    // A square in pixels, so the crop keeps the face's proportions rather than
    // the region's aspect ratio.
    let side = (region.width * image_width).max(region.height * image_height) * margin.max(1.0);

    let mut left = center_x - side / 2.0;
    let mut top = center_y - side / 2.0;
    let mut right = left + side;
    let mut bottom = top + side;
    // Clamp into the image, then pull the opposite edge in so the crop stays
    // square.
    if left < 0.0 {
        right -= left;
        left = 0.0;
    }
    if top < 0.0 {
        bottom -= top;
        top = 0.0;
    }
    if right > image_width {
        left -= right - image_width;
        right = image_width;
    }
    if bottom > image_height {
        top -= bottom - image_height;
        bottom = image_height;
    }
    left = left.max(0.0);
    top = top.max(0.0);
    let width = (right - left).max(1.0);
    let height = (bottom - top).max(1.0);

    let mut data = vec![0u8; size as usize * size as usize * 3];
    for y in 0..size {
        let source_y = top + (y as f32 + 0.5) / size as f32 * height - 0.5;
        let y0 = source_y.floor();
        let wy = source_y - y0;
        for x in 0..size {
            let source_x = left + (x as f32 + 0.5) / size as f32 * width - 0.5;
            let x0 = source_x.floor();
            let wx = source_x - x0;
            let index = (y as usize * size as usize + x as usize) * 3;
            for channel in 0..3 {
                let sample = |cx: i64, cy: i64| -> f32 {
                    let cx = cx.clamp(0, source.width as i64 - 1) as usize;
                    let cy = cy.clamp(0, source.height as i64 - 1) as usize;
                    f32::from(source.data[(cy * source.width as usize + cx) * 3 + channel])
                };
                let x0i = x0 as i64;
                let y0i = y0 as i64;
                let top_sample = sample(x0i, y0i) * (1.0 - wx) + sample(x0i + 1, y0i) * wx;
                let bottom_sample =
                    sample(x0i, y0i + 1) * (1.0 - wx) + sample(x0i + 1, y0i + 1) * wx;
                data[index + channel] = (top_sample * (1.0 - wy) + bottom_sample * wy)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }

    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality.clamp(1, 100)).encode(
        &data,
        size,
        size,
        image::ExtendedColorType::Rgb8,
    )?;
    Ok((bytes, size, size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage};

    #[test]
    fn decodes_a_png_artifact_to_rgb8() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preview.png");
        let mut source = RgbImage::new(4, 3);
        source.put_pixel(1, 2, Rgb([10, 20, 30]));
        source.save_with_format(&path, ImageFormat::Png).unwrap();

        let pixels = decode_rgb_pixels(&path).unwrap();
        assert_eq!((pixels.width, pixels.height), (4, 3));
        assert_eq!(pixels.data.len(), 4 * 3 * 3);
        let index = (2 * 4 + 1) * 3;
        assert_eq!(&pixels.data[index..index + 3], &[10, 20, 30]);
    }

    #[test]
    fn rejects_buffers_that_do_not_match_the_size() {
        assert!(RgbPixels::new(2, 2, vec![0; 11]).is_err());
        assert!(RgbPixels::new(0, 0, Vec::new()).is_err());
        assert!(RgbPixels::new(1, 1, vec![0, 0, 0]).is_ok());
    }

    #[test]
    fn missing_file_is_an_io_error_not_a_panic() {
        let directory = tempfile::tempdir().unwrap();
        assert!(decode_rgb_pixels(&directory.path().join("absent.png")).is_err());
    }
}

#[cfg(test)]
mod face_crop_tests {
    use super::*;
    use oxy_domain::NormalizedRect;

    /// Left half dark, right half bright, so a crop's position is observable.
    fn split_image(width: u32, height: u32) -> RgbPixels {
        let mut data = vec![0u8; width as usize * height as usize * 3];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let value = if (x as u32) < width / 2 { 20 } else { 230 };
                let index = (y * width as usize + x) * 3;
                data[index] = value;
                data[index + 1] = value;
                data[index + 2] = value;
            }
        }
        RgbPixels::new(width, height, data).unwrap()
    }

    fn average(pixels: &[u8]) -> f32 {
        pixels.iter().map(|value| f32::from(*value)).sum::<f32>() / pixels.len() as f32
    }

    #[test]
    fn crops_the_requested_region_and_scales_it_to_the_requested_size() {
        let source = split_image(200, 100);
        // The whole left (dark) half.
        let (bytes, width, height) = face_crop_jpeg(
            &source,
            NormalizedRect::new(0.0, 0.0, 0.5, 1.0),
            64,
            1.0,
            90,
        )
        .unwrap();
        assert_eq!((width, height), (64, 64));
        assert!(!bytes.is_empty());
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!((decoded.width(), decoded.height()), (64, 64));
        assert!(
            average(decoded.as_raw()) < 60.0,
            "the dark half must stay dark, got {}",
            average(decoded.as_raw())
        );
    }

    #[test]
    fn the_crop_follows_the_region() {
        let source = split_image(200, 100);
        let (dark, _, _) = face_crop_jpeg(
            &source,
            NormalizedRect::new(0.0, 0.0, 0.4, 1.0),
            32,
            1.0,
            90,
        )
        .unwrap();
        let (bright, _, _) = face_crop_jpeg(
            &source,
            NormalizedRect::new(0.6, 0.0, 0.4, 1.0),
            32,
            1.0,
            90,
        )
        .unwrap();
        let dark = average(image::load_from_memory(&dark).unwrap().to_rgb8().as_raw());
        let bright = average(image::load_from_memory(&bright).unwrap().to_rgb8().as_raw());
        assert!(bright > dark + 100.0, "dark {dark} bright {bright}");
    }

    #[test]
    fn a_region_at_the_edge_is_clamped_rather_than_padded() {
        let source = split_image(200, 100);
        // A face straddling the top-left corner: the square cannot extend past
        // the image, so the crop shifts inward.
        let (bytes, width, height) = face_crop_jpeg(
            &source,
            NormalizedRect::new(0.0, 0.0, 0.1, 0.2),
            48,
            1.4,
            90,
        )
        .unwrap();
        assert_eq!((width, height), (48, 48));
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!((decoded.width(), decoded.height()), (48, 48));
        // The top-left is dark, so a corner-anchored crop is mostly dark.
        assert!(average(decoded.as_raw()) < 100.0);
    }

    #[test]
    fn margin_widens_the_square() {
        let source = split_image(200, 100);
        // Centered in the bright half: a tight crop stays entirely bright, and
        // a wide one reaches across the boundary into the dark half.
        let region = NormalizedRect::new(0.55, 0.45, 0.1, 0.1);
        let dark_fraction = |margin: f32| {
            let (bytes, _, _) = face_crop_jpeg(&source, region, 32, margin, 90).unwrap();
            let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
            let dark = decoded
                .as_raw()
                .chunks_exact(3)
                .filter(|pixel| pixel[0] < 128)
                .count();
            dark as f32 / (decoded.width() * decoded.height()) as f32
        };
        assert_eq!(
            dark_fraction(1.0),
            0.0,
            "a tight crop stays inside the bright half"
        );
        assert!(
            dark_fraction(3.0) > 0.05,
            "a wide crop must reach the dark half, got {}",
            dark_fraction(3.0)
        );
    }

    #[test]
    fn degenerate_input_never_panics() {
        let source = split_image(16, 16);
        assert!(
            face_crop_jpeg(
                &source,
                NormalizedRect::new(0.1, 0.1, 0.0, 0.2),
                32,
                1.4,
                90
            )
            .is_err()
        );
        assert!(
            face_crop_jpeg(&source, NormalizedRect::new(0.1, 0.1, 0.2, 0.2), 0, 1.4, 90).is_err()
        );
        // A square that would exceed the image on both axes is still produced.
        let (bytes, _, _) = face_crop_jpeg(
            &source,
            NormalizedRect::new(0.0, 0.0, 1.0, 1.0),
            24,
            1.0,
            90,
        )
        .unwrap();
        assert!(!bytes.is_empty());
    }
}

#[cfg(test)]
mod orientation_tests {
    use super::*;

    /// 2x1: left red, right blue. Both a rotation and a flip change the order.
    fn pair() -> RgbPixels {
        RgbPixels::new(2, 1, vec![255, 0, 0, 0, 0, 255]).unwrap()
    }

    fn left(pixels: &RgbPixels) -> [u8; 3] {
        [pixels.data[0], pixels.data[1], pixels.data[2]]
    }

    #[test]
    fn an_upright_or_unknown_orientation_is_a_no_op() {
        let unchanged = apply_exif_orientation(pair(), 1).unwrap();
        assert_eq!((unchanged.width, unchanged.height), (2, 1));
        assert_eq!(left(&unchanged), [255, 0, 0]);

        // 0 and 9 are not valid EXIF orientations; leaving the pixels alone is
        // safer than guessing a rotation.
        for value in [0, 9, 255] {
            let same = apply_exif_orientation(pair(), value).unwrap();
            assert_eq!((same.width, same.height), (2, 1));
            assert_eq!(left(&same), [255, 0, 0], "orientation {value}");
        }
    }

    #[test]
    fn horizontal_flip_reverses_the_pixel_order() {
        let flipped = apply_exif_orientation(pair(), 2).unwrap();
        assert_eq!((flipped.width, flipped.height), (2, 1));
        assert_eq!(left(&flipped), [0, 0, 255]);
    }

    #[test]
    fn a_quarter_turn_swaps_the_dimensions() {
        let rotated = apply_exif_orientation(pair(), 6).unwrap();
        assert_eq!(
            (rotated.width, rotated.height),
            (1, 2),
            "a 90 degree turn must swap width and height"
        );
        let turned = apply_exif_orientation(pair(), 8).unwrap();
        assert_eq!((turned.width, turned.height), (1, 2));
        // The two directions are not the same transform.
        assert_ne!(rotated.data, turned.data);
    }

    #[test]
    fn a_half_turn_keeps_the_dimensions_and_reverses_both_axes() {
        let turned = apply_exif_orientation(pair(), 3).unwrap();
        assert_eq!((turned.width, turned.height), (2, 1));
        assert_eq!(left(&turned), [0, 0, 255]);
    }

    #[test]
    fn every_defined_orientation_round_trips_in_area() {
        for value in 1..=8u8 {
            let pixels = pair();
            let area = pixels.width * pixels.height;
            let oriented = apply_exif_orientation(pixels, value).unwrap();
            assert_eq!(
                oriented.width * oriented.height,
                area,
                "orientation {value} must not lose or invent pixels"
            );
        }
    }
}
