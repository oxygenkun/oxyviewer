//! Minimal RGB8 image buffer and the resampling the face analyzer needs.
//!
//! The analyzer deliberately owns its own buffer type instead of depending on a
//! decoder crate: the Host decodes a preview and hands over pixels, so this
//! crate has no filesystem or format knowledge and can run under any decoder.

use crate::FaceError;

/// Row-major, top-left origin, three bytes per pixel in RGB order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbImage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl RgbImage {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self, FaceError> {
        let expected = width as usize * height as usize * 3;
        if width == 0 || height == 0 || data.len() != expected {
            return Err(FaceError::InvalidImage {
                width,
                height,
                bytes: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// Copies a sub-rectangle. The rectangle is clamped to the image, so a
    /// caller cannot read outside it.
    pub fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> RgbImage {
        let x = x.min(self.width.saturating_sub(1));
        let y = y.min(self.height.saturating_sub(1));
        let width = width.min(self.width - x).max(1);
        let height = height.min(self.height - y).max(1);
        let mut data = vec![0u8; width as usize * height as usize * 3];
        for row in 0..height as usize {
            let source = ((y as usize + row) * self.width as usize + x as usize) * 3;
            let target = row * width as usize * 3;
            let bytes = width as usize * 3;
            data[target..target + bytes].copy_from_slice(&self.data[source..source + bytes]);
        }
        RgbImage {
            width,
            height,
            data,
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let index = (y as usize * self.width as usize + x as usize) * 3;
        [self.data[index], self.data[index + 1], self.data[index + 2]]
    }

    /// Converts to a `1x3xHxW` float tensor. Face models in the OpenCV
    /// ecosystem take raw `0..=255` values with no mean subtraction and no
    /// scaling, so channel order is the only thing that differs between them.
    pub fn to_nchw_f32(&self, order: ChannelOrder) -> Vec<f32> {
        let plane = self.width as usize * self.height as usize;
        let mut tensor = vec![0.0f32; plane * 3];
        for (index, pixel) in self.data.chunks_exact(3).enumerate() {
            let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
            let (c0, c1, c2) = match order {
                ChannelOrder::Rgb => (r, g, b),
                ChannelOrder::Bgr => (b, g, r),
            };
            tensor[index] = f32::from(c0);
            tensor[plane + index] = f32::from(c1);
            tensor[plane * 2 + index] = f32::from(c2);
        }
        tensor
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelOrder {
    Rgb,
    Bgr,
}

/// A resized and zero-padded square image plus the mapping back to source
/// pixels.
///
/// Padding is added on the right and bottom only, so `offset_x`/`offset_y` are
/// zero today; they are kept explicit because the mapping, not the padding
/// strategy, is what callers depend on.
#[derive(Debug, Clone)]
pub struct Letterboxed {
    pub image: RgbImage,
    pub source_width: u32,
    pub source_height: u32,
    /// Source pixels per letterbox pixel along each axis.
    pub scale_x: f32,
    pub scale_y: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

impl Letterboxed {
    /// Maps a rectangle measured in letterbox pixels back to source pixels.
    pub fn to_source_rect(&self, x: f32, y: f32, width: f32, height: f32) -> (f32, f32, f32, f32) {
        (
            (x - self.offset_x) / self.scale_x,
            (y - self.offset_y) / self.scale_y,
            width / self.scale_x,
            height / self.scale_y,
        )
    }

    /// Maps a point measured in letterbox pixels back to source pixels.
    pub fn to_source_point(&self, x: f32, y: f32) -> (f32, f32) {
        (
            (x - self.offset_x) / self.scale_x,
            (y - self.offset_y) / self.scale_y,
        )
    }
}

/// Aspect-preserving resize into a square canvas with black padding on the
/// right and bottom, mirroring `copyMakeBorder(..., BORDER_CONSTANT, 0)`.
pub fn letterbox(source: &RgbImage, target: u32) -> Letterboxed {
    let fit = (target as f32 / source.width as f32).min(target as f32 / source.height as f32);
    let scaled_width = ((source.width as f32 * fit).round() as u32).clamp(1, target);
    let scaled_height = ((source.height as f32 * fit).round() as u32).clamp(1, target);
    let resized = resize_bilinear(source, scaled_width, scaled_height);

    let mut data = vec![0u8; target as usize * target as usize * 3];
    for y in 0..scaled_height as usize {
        let source_row = y * scaled_width as usize * 3;
        let target_row = y * target as usize * 3;
        let row_bytes = scaled_width as usize * 3;
        data[target_row..target_row + row_bytes]
            .copy_from_slice(&resized.data[source_row..source_row + row_bytes]);
    }

    Letterboxed {
        image: RgbImage {
            width: target,
            height: target,
            data,
        },
        source_width: source.width,
        source_height: source.height,
        scale_x: scaled_width as f32 / source.width as f32,
        scale_y: scaled_height as f32 / source.height as f32,
        offset_x: 0.0,
        offset_y: 0.0,
    }
}

/// One detector input derived from a source image, plus the mapping back to
/// source pixels.
///
/// A view is either the whole frame or a tile of it. Tiles exist because a
/// group photo downscaled into a single detector input loses the small faces
/// that matter most there; each tile keeps a smaller region at a higher
/// effective resolution.
#[derive(Debug, Clone)]
pub struct AnalysisView {
    pub boxed: Letterboxed,
    /// Source-pixel origin of this view. `(0, 0)` for the whole frame.
    pub origin_x: f32,
    pub origin_y: f32,
}

impl AnalysisView {
    pub fn to_source_rect(&self, x: f32, y: f32, width: f32, height: f32) -> (f32, f32, f32, f32) {
        let (sx, sy, sw, sh) = self.boxed.to_source_rect(x, y, width, height);
        (sx + self.origin_x, sy + self.origin_y, sw, sh)
    }

    pub fn to_source_point(&self, x: f32, y: f32) -> (f32, f32) {
        let (sx, sy) = self.boxed.to_source_point(x, y);
        (sx + self.origin_x, sy + self.origin_y)
    }
}

/// Views to run detection over: the whole frame first, then tiles when the
/// source is large enough for downscaling to matter.
///
/// Tiling is skipped below `target * 1.5` on the long side, where a tile would
/// not raise resolution enough to pay for the extra pass. Above that the grid
/// grows to 2x2, and to 3x3 once the frame is more than four times the detector
/// input — 3x3 at that point already keeps each tile under 1.5x the input, so a
/// fourth pass would cost inference without recovering more faces. Tiles
/// overlap so a face on a seam is still wholly inside one of them, and the
/// whole-frame view is always included because a single tile can miss a large
/// face that spans several tiles.
pub fn analysis_views(source: &RgbImage, target: u32, tiled: bool) -> Vec<AnalysisView> {
    let full = AnalysisView {
        boxed: letterbox(source, target),
        origin_x: 0.0,
        origin_y: 0.0,
    };
    let long_side = source.width.max(source.height) as f32;
    if !tiled || long_side <= target as f32 * 1.5 {
        return vec![full];
    }
    let grid = if long_side <= target as f32 * 4.0 {
        2
    } else {
        3
    };
    let mut views = vec![full];
    let step_x = source.width as f32 / grid as f32;
    let step_y = source.height as f32 / grid as f32;
    // 15% overlap: enough that a face crossing a seam sits inside a neighbour.
    let tile_width = (step_x * 1.15).ceil().min(source.width as f32) as u32;
    let tile_height = (step_y * 1.15).ceil().min(source.height as f32) as u32;
    for row in 0..grid {
        for column in 0..grid {
            let x = (column as f32 * step_x)
                .min((source.width - tile_width) as f32)
                .max(0.0) as u32;
            let y = (row as f32 * step_y)
                .min((source.height - tile_height) as f32)
                .max(0.0) as u32;
            views.push(AnalysisView {
                boxed: letterbox(&source.crop(x, y, tile_width, tile_height), target),
                origin_x: x as f32,
                origin_y: y as f32,
            });
        }
    }
    views
}

/// Bilinear resample using pixel-center coordinates, matching the sampling
/// convention of `cv::resize(..., INTER_LINEAR)`.
pub fn resize_bilinear(source: &RgbImage, width: u32, height: u32) -> RgbImage {
    if width == source.width && height == source.height {
        return source.clone();
    }
    let mut data = vec![0u8; width as usize * height as usize * 3];
    let scale_x = source.width as f32 / width as f32;
    let scale_y = source.height as f32 / height as f32;
    for y in 0..height {
        let source_y = (y as f32 + 0.5) * scale_y - 0.5;
        let y0 = source_y.floor();
        let wy = source_y - y0;
        for x in 0..width {
            let source_x = (x as f32 + 0.5) * scale_x - 0.5;
            let x0 = source_x.floor();
            let wx = source_x - x0;
            let index = (y as usize * width as usize + x as usize) * 3;
            for channel in 0..3 {
                let sample = |cx: i64, cy: i64| -> f32 {
                    let cx = cx.clamp(0, source.width as i64 - 1) as u32;
                    let cy = cy.clamp(0, source.height as i64 - 1) as u32;
                    f32::from(source.pixel(cx, cy)[channel])
                };
                let x0i = x0 as i64;
                let y0i = y0 as i64;
                let top = sample(x0i, y0i) * (1.0 - wx) + sample(x0i + 1, y0i) * wx;
                let bottom = sample(x0i, y0i + 1) * (1.0 - wx) + sample(x0i + 1, y0i + 1) * wx;
                data[index + channel] =
                    (top * (1.0 - wy) + bottom * wy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    RgbImage {
        width,
        height,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, color: [u8; 3]) -> RgbImage {
        let mut data = Vec::with_capacity(width as usize * height as usize * 3);
        for _ in 0..width * height {
            data.extend_from_slice(&color);
        }
        RgbImage::new(width, height, data).unwrap()
    }

    #[test]
    fn rejects_buffers_that_do_not_match_the_size() {
        assert!(matches!(
            RgbImage::new(2, 2, vec![0; 11]),
            Err(FaceError::InvalidImage { .. })
        ));
        assert!(RgbImage::new(0, 2, vec![]).is_err());
    }

    #[test]
    fn nchw_conversion_orders_channels() {
        let image = RgbImage::new(1, 1, vec![10, 20, 30]).unwrap();
        assert_eq!(image.to_nchw_f32(ChannelOrder::Rgb), vec![10.0, 20.0, 30.0]);
        assert_eq!(image.to_nchw_f32(ChannelOrder::Bgr), vec![30.0, 20.0, 10.0]);
    }

    #[test]
    fn letterbox_preserves_aspect_and_pads_with_black() {
        let source = solid(400, 200, [255, 255, 255]);
        let boxed = letterbox(&source, 640);
        assert_eq!(boxed.image.width(), 640);
        assert_eq!(boxed.image.height(), 640);
        // 400x200 scaled by 1.6 -> 640x320, bottom 320 rows stay black.
        assert!((boxed.scale_x - 1.6).abs() < 1e-6);
        assert_eq!(boxed.image.pixel(10, 10), [255, 255, 255]);
        assert_eq!(boxed.image.pixel(10, 400), [0, 0, 0]);
        let (x, y, width, height) = boxed.to_source_rect(160.0, 80.0, 64.0, 32.0);
        assert!((x - 100.0).abs() < 1e-3);
        assert!((y - 50.0).abs() < 1e-3);
        assert!((width - 40.0).abs() < 1e-3);
        assert!((height - 20.0).abs() < 1e-3);
    }

    #[test]
    fn letterbox_of_a_portrait_pads_the_right_edge() {
        let source = solid(200, 400, [128, 128, 128]);
        let boxed = letterbox(&source, 640);
        assert!((boxed.scale_y - 1.6).abs() < 1e-6);
        assert_eq!(boxed.image.pixel(10, 10), [128, 128, 128]);
        assert_eq!(boxed.image.pixel(400, 10), [0, 0, 0]);
    }

    #[test]
    fn resize_keeps_a_solid_color_exact() {
        let source = solid(100, 50, [7, 8, 9]);
        let resized = resize_bilinear(&source, 640, 640);
        assert_eq!(resized.pixel(0, 0), [7, 8, 9]);
        assert_eq!(resized.pixel(639, 639), [7, 8, 9]);
    }

    #[test]
    fn resize_uses_pixel_center_sampling() {
        // Two columns: black then white. Scaling down to one column must
        // average them to the mid-point, not snap to a neighbor.
        let data = vec![0, 0, 0, 255, 255, 255];
        let source = RgbImage::new(2, 1, data).unwrap();
        let resized = resize_bilinear(&source, 1, 1);
        assert!(resized.pixel(0, 0)[0] >= 127 && resized.pixel(0, 0)[0] <= 128);
    }
}
