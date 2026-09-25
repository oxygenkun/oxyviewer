//! Model-specific face tensors. These do not assign or confirm identity.

use image::{RgbImage, imageops::FilterType};
use thiserror::Error;

const FACE_EDGE: usize = 112;
const ARCFACE_TEMPLATE: [[f64; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FaceInputError {
    #[error("face input image or detector canvas is invalid")]
    InvalidDimensions,
    #[error("face landmarks are invalid or degenerate")]
    InvalidLandmarks,
}

#[derive(Debug)]
pub struct ScrfdInput {
    /// NCHW RGB float32, 1 x 3 x size x size.
    pub values: Vec<f32>,
    /// Map model-space boxes and landmarks back into source pixel coordinates.
    pub scale: f32,
    pub resized_width: u32,
    pub resized_height: u32,
    pub canvas_size: u32,
}

/// InsightFace SCRFD preprocessing: preserve aspect ratio, pad on the right
/// and bottom, then RGB `(pixel - 127.5) / 128` in channel-first order.
pub fn scrfd_input(image: &RgbImage, canvas_size: u32) -> Result<ScrfdInput, FaceInputError> {
    if image.width() == 0 || image.height() == 0 || !(320..=1280).contains(&canvas_size) {
        return Err(FaceInputError::InvalidDimensions);
    }
    let edge = image.width().max(image.height());
    let scale = canvas_size as f32 / edge as f32;
    let width = ((image.width() as f64 * canvas_size as f64) / edge as f64) as u32;
    let height = ((image.height() as f64 * canvas_size as f64) / edge as f64) as u32;
    if width == 0 || height == 0 {
        return Err(FaceInputError::InvalidDimensions);
    }
    let resized = image::imageops::resize(image, width, height, FilterType::Triangle);
    let side = canvas_size as usize;
    let plane = side * side;
    // Zero-padded BGR image enters blobFromImage with swapRB=true. Its zero
    // padding becomes -127.5/128 in the resulting RGB float tensor.
    let mut values = vec![-127.5 / 128.0; 3 * plane];
    for y in 0..height {
        for x in 0..width {
            let pixel = resized.get_pixel(x, y).0;
            let offset = y as usize * side + x as usize;
            for channel in 0..3 {
                values[channel * plane + offset] = (pixel[channel] as f32 - 127.5) / 128.0;
            }
        }
    }
    Ok(ScrfdInput {
        values,
        scale,
        resized_width: width,
        resized_height: height,
        canvas_size,
    })
}

/// Align five SCRFD landmarks to the 112-pixel ArcFace template and produce
/// AdaFace's NCHW BGR float32 input `(pixel - 127.5) / 127.5`.
pub fn adaface_input(
    image: &RgbImage,
    landmarks: [[f32; 2]; 5],
) -> Result<Vec<f32>, FaceInputError> {
    if image.width() == 0 || image.height() == 0 {
        return Err(FaceInputError::InvalidDimensions);
    }
    if landmarks.iter().flatten().any(|value| !value.is_finite()) {
        return Err(FaceInputError::InvalidLandmarks);
    }
    let source_center = landmarks.iter().fold([0.0_f64; 2], |mut sum, point| {
        sum[0] += f64::from(point[0]) / 5.0;
        sum[1] += f64::from(point[1]) / 5.0;
        sum
    });
    let target_center = ARCFACE_TEMPLATE
        .iter()
        .fold([0.0_f64; 2], |mut sum, point| {
            sum[0] += point[0] / 5.0;
            sum[1] += point[1] / 5.0;
            sum
        });
    let mut denominator = 0.0;
    let mut real = 0.0;
    let mut imaginary = 0.0;
    for (source, target) in landmarks.iter().zip(ARCFACE_TEMPLATE) {
        let sx = f64::from(source[0]) - source_center[0];
        let sy = f64::from(source[1]) - source_center[1];
        let tx = target[0] - target_center[0];
        let ty = target[1] - target_center[1];
        denominator += sx * sx + sy * sy;
        real += sx * tx + sy * ty;
        imaginary += sx * ty - sy * tx;
    }
    if denominator < 1e-6 {
        return Err(FaceInputError::InvalidLandmarks);
    }
    let a = real / denominator;
    let b = imaginary / denominator;
    let determinant = a * a + b * b;
    if determinant < 1e-8 || !determinant.is_finite() {
        return Err(FaceInputError::InvalidLandmarks);
    }
    let translate_x = target_center[0] - a * source_center[0] + b * source_center[1];
    let translate_y = target_center[1] - b * source_center[0] - a * source_center[1];
    let plane = FACE_EDGE * FACE_EDGE;
    let mut values = vec![0.0; 3 * plane];
    for y in 0..FACE_EDGE {
        for x in 0..FACE_EDGE {
            let u = x as f64 - translate_x;
            let v = y as f64 - translate_y;
            let source_x = (a * u + b * v) / determinant;
            let source_y = (-b * u + a * v) / determinant;
            let [red, green, blue] = bilinear_rgb(image, source_x, source_y);
            let offset = y * FACE_EDGE + x;
            for (channel, value) in [blue, green, red].into_iter().enumerate() {
                values[channel * plane + offset] = (value - 127.5) / 127.5;
            }
        }
    }
    Ok(values)
}

fn bilinear_rgb(image: &RgbImage, x: f64, y: f64) -> [f32; 3] {
    let x0 = x.floor();
    let y0 = y.floor();
    let dx = (x - x0) as f32;
    let dy = (y - y0) as f32;
    let mut output = [0.0; 3];
    for (ox, wx) in [(0_i64, 1.0 - dx), (1, dx)] {
        for (oy, wy) in [(0_i64, 1.0 - dy), (1, dy)] {
            let px = x0 as i64 + ox;
            let py = y0 as i64 + oy;
            if px >= 0 && py >= 0 && px < i64::from(image.width()) && py < i64::from(image.height())
            {
                let sample = image.get_pixel(px as u32, py as u32).0;
                let weight = wx * wy;
                for channel in 0..3 {
                    output[channel] += weight * f32::from(sample[channel]);
                }
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrfd_canvas_preserves_aspect_and_zero_padding() {
        let mut image = RgbImage::new(4, 2);
        for pixel in image.pixels_mut() {
            pixel.0 = [255, 128, 0];
        }
        let input = scrfd_input(&image, 320).unwrap();
        assert_eq!((input.resized_width, input.resized_height), (320, 160));
        assert_eq!(input.scale, 80.0);
        assert_eq!(input.values.len(), 3 * 320 * 320);
        assert!((input.values[0] - 1.0).abs() < 0.004);
        assert!((input.values[320 * 160] + 127.5 / 128.0).abs() < 0.0001);
        assert!((input.values[2 * 320 * 320] + 127.5 / 128.0).abs() < 0.0001);
    }

    #[test]
    fn scrfd_tensor_matches_research_opencv_reference_samples() {
        let image = RgbImage::from_fn(180, 160, |x, y| {
            image::Rgb([
                ((x * 3 + y * 7) % 256) as u8,
                ((x * 3 + y * 7 + 40) % 256) as u8,
                ((x * 3 + y * 7 + 80) % 256) as u8,
            ])
        });
        let input = scrfd_input(&image, 320).unwrap();
        assert_eq!((input.resized_width, input.resized_height), (320, 284));
        for (channel, y, x, expected) in [
            (0, 20, 20, -0.13671875_f32),
            (1, 140, 160, -0.27734375),
            (2, 250, 200, -0.05078125),
            (0, 300, 300, -0.99609375),
        ] {
            let actual = input.values[channel * 320 * 320 + y * 320 + x];
            assert!(
                (actual - expected).abs() < 0.04,
                "channel={channel}, x={x}, y={y}, actual={actual}, expected={expected}"
            );
        }
    }

    #[test]
    fn adaface_alignment_uses_bgr_and_rejects_degenerate_landmarks() {
        let mut image = RgbImage::new(112, 112);
        for pixel in image.pixels_mut() {
            pixel.0 = [255, 128, 0];
        }
        let landmarks = ARCFACE_TEMPLATE.map(|point| [point[0] as f32, point[1] as f32]);
        let tensor = adaface_input(&image, landmarks).unwrap();
        let center = 56 * 112 + 56;
        assert!((tensor[center] + 1.0).abs() < 0.001);
        assert!((tensor[112 * 112 + center] - (0.5 / 127.5)).abs() < 0.001);
        assert!((tensor[2 * 112 * 112 + center] - 1.0).abs() < 0.001);
        assert_eq!(
            adaface_input(&image, [[10.0, 10.0]; 5]),
            Err(FaceInputError::InvalidLandmarks)
        );
    }

    #[test]
    fn adaface_warp_matches_research_opencv_reference_samples() {
        let image = RgbImage::from_fn(180, 160, |x, y| {
            image::Rgb([
                ((x * 3 + y * 7) % 256) as u8,
                ((x * 3 + y * 7 + 40) % 256) as u8,
                ((x * 3 + y * 7 + 80) % 256) as u8,
            ])
        });
        let landmarks = ARCFACE_TEMPLATE.map(|point| {
            [
                (point[0] * 1.3 + 12.0) as f32,
                (point[1] * 1.3 + 8.0) as f32,
            ]
        });
        let tensor = adaface_input(&image, landmarks).unwrap();
        // Generated with the research environment's InsightFace norm_crop and
        // cv2.dnn.blobFromImage(swapRB=false). OpenCV quantizes interpolation.
        for (channel, y, x, expected) in [
            (0, 20, 20, 0.3803922_f32),
            (1, 56, 56, -0.2784314),
            (2, 80, 70, -0.4588236),
            (0, 0, 0, 0.3490196),
        ] {
            let actual = tensor[channel * FACE_EDGE * FACE_EDGE + y * FACE_EDGE + x];
            assert!(
                (actual - expected).abs() < 0.03,
                "channel={channel}, x={x}, y={y}, actual={actual}, expected={expected}"
            );
        }
    }
}
