//! Small native drag images composed from an already prepared thumbnail.
use crate::{MediaError, types::PreviewGeometry};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Rgba, RgbaImage, imageops};
use std::io::Cursor;

const PHOTO_EDGE: u32 = 128;
const BORDER: u32 = 4;
const STACK_STEP: u32 = 8;

/// Call off the UI thread. Missing/corrupt previews use a photo placeholder;
/// dragging an otherwise valid original must not depend on decoder support.
pub fn photo_drag_image(
    encoded: Option<&[u8]>,
    geometry: Option<PreviewGeometry>,
    count: usize,
) -> Result<Vec<u8>, MediaError> {
    let photo = encoded
        .and_then(|bytes| decode_thumbnail(bytes, geometry).ok())
        .unwrap_or_else(placeholder);
    let layers = count.clamp(1, 3) as u32;
    let width = photo.width() + BORDER * 2;
    let height = photo.height() + BORDER * 2;
    let stack = (layers - 1) * STACK_STEP;
    let mut canvas = RgbaImage::new(width + stack + 4, height + stack + 4);
    for layer in (0..layers).rev() {
        let offset = layer * STACK_STEP;
        fill(
            &mut canvas,
            offset + 3,
            offset + 4,
            width,
            height,
            Rgba([0, 0, 0, 65]),
        );
        fill(
            &mut canvas,
            offset,
            offset,
            width,
            height,
            Rgba([250, 250, 250, 255]),
        );
        imageops::overlay(
            &mut canvas,
            &photo,
            (offset + BORDER).into(),
            (offset + BORDER).into(),
        );
    }
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(canvas).write_to(&mut png, ImageFormat::Png)?;
    Ok(png.into_inner())
}

fn decode_thumbnail(
    bytes: &[u8],
    geometry: Option<PreviewGeometry>,
) -> Result<RgbaImage, MediaError> {
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(MediaError::InvalidMediaRequest);
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    limits.max_alloc = Some(8 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    let geometry = geometry.filter(|geometry| {
        let rect = geometry.content_rect;
        geometry.display_size.width > 0
            && geometry.display_size.height > 0
            && rect.width > 0
            && rect.height > 0
            && rect
                .x
                .checked_add(rect.width)
                .is_some_and(|right| right <= image.width())
            && rect
                .y
                .checked_add(rect.height)
                .is_some_and(|bottom| bottom <= image.height())
    });
    let (display_width, display_height) = if let Some(geometry) = geometry {
        let rect = geometry.content_rect;
        image = image.crop_imm(rect.x, rect.y, rect.width, rect.height);
        (geometry.display_size.width, geometry.display_size.height)
    } else {
        (image.width(), image.height())
    };
    let scale = f64::from(PHOTO_EDGE) / f64::from(display_width.max(display_height));
    let width = (f64::from(display_width) * scale).round().max(1.0) as u32;
    let height = (f64::from(display_height) * scale).round().max(1.0) as u32;
    Ok(image
        .resize_exact(width, height, imageops::FilterType::Triangle)
        .into_rgba8())
}

fn fill(canvas: &mut RgbaImage, x: u32, y: u32, width: u32, height: u32, color: Rgba<u8>) {
    for row in y..y + height {
        for column in x..x + width {
            canvas.put_pixel(column, row, color);
        }
    }
}

fn placeholder() -> RgbaImage {
    RgbaImage::from_fn(PHOTO_EDGE, 96, |x, y| {
        if y > 90_u32.saturating_sub(x / 2) || y > 32 + x / 3 {
            Rgba([90, 130, 145, 255])
        } else if (i64::from(x) - 96).pow(2) + (i64::from(y) - 22).pow(2) < 64 {
            Rgba([245, 221, 162, 255])
        } else {
            Rgba([187, 212, 222, 255])
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PreviewContentRect, PreviewDisplaySize};

    fn encoded(image: RgbaImage) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn uses_photo_pixels_with_transparent_surroundings_and_a_white_frame() {
        let bytes = encoded(RgbaImage::from_pixel(80, 40, Rgba([220, 35, 70, 255])));
        let png = photo_drag_image(Some(&bytes), None, 1).unwrap();
        let image = image::load_from_memory(&png).unwrap().into_rgba8();
        assert_eq!(image.dimensions(), (140, 76));
        assert_eq!(image.get_pixel(10, 10).0, [220, 35, 70, 255]);
        assert_eq!(image.get_pixel(0, 0).0, [250, 250, 250, 255]);
        assert_eq!(image.get_pixel(139, 0).0[3], 0);
    }

    #[test]
    fn multiple_files_show_separate_stacked_photo_frames_with_bounded_size() {
        let bytes = encoded(RgbaImage::from_pixel(40, 80, Rgba([20, 100, 200, 255])));
        let render = |count| {
            image::load_from_memory(&photo_drag_image(Some(&bytes), None, count).unwrap())
                .unwrap()
                .into_rgba8()
        };
        let single = render(1);
        let pair = render(2);
        let many = render(1000);
        assert_eq!(pair.dimensions(), (single.width() + 8, single.height() + 8));
        assert_eq!(
            many.dimensions(),
            (single.width() + 16, single.height() + 16)
        );
        assert_eq!(pair.get_pixel(77, 12).0, [250, 250, 250, 255]);
        assert_eq!(many.get_pixel(85, 20).0, [250, 250, 250, 255]);
    }

    #[test]
    fn crops_preview_padding_and_preserves_the_display_aspect() {
        let mut pixels = RgbaImage::from_pixel(100, 100, Rgba([0, 0, 0, 255]));
        fill(&mut pixels, 20, 10, 60, 80, Rgba([30, 200, 90, 255]));
        let bytes = encoded(pixels);
        let geometry = PreviewGeometry {
            content_rect: PreviewContentRect {
                x: 20,
                y: 10,
                width: 60,
                height: 80,
            },
            display_size: PreviewDisplaySize {
                width: 300,
                height: 400,
            },
        };
        let png = photo_drag_image(Some(&bytes), Some(geometry), 1).unwrap();
        let image = image::load_from_memory(&png).unwrap().into_rgba8();
        assert_eq!(image.dimensions(), (108, 140));
        assert_eq!(image.get_pixel(4, 4).0, [30, 200, 90, 255]);
        assert_eq!(image.get_pixel(99, 131).0, [30, 200, 90, 255]);
    }

    #[test]
    fn unavailable_or_corrupt_thumbnails_still_produce_photo_cards() {
        assert_eq!(
            photo_drag_image(None, None, 2).unwrap(),
            photo_drag_image(Some(b"bad thumbnail"), None, 2).unwrap()
        );
    }
}
