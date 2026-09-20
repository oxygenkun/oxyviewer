//! 5-landmark similarity alignment.
//!
//! The transform is the least-squares similarity (rotation, uniform scale,
//! translation) that maps detected landmarks onto the canonical ArcFace
//! positions used by ArcFace-family embedders. This reproduces the result of OpenCV's
//! `FaceRecognizerSF::alignCrop`, which estimates the same transform and then
//! applies `warpAffine(..., INTER_LINEAR)`.

use crate::image::RgbImage;

/// Canonical 112x112 landmark positions: right eye, left eye, nose tip, right
/// mouth corner, left mouth corner.
pub const REFERENCE_LANDMARKS_112: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// An affine transform `dst = M * (src, 1)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimilarityTransform {
    /// `[[a, b, tx], [c, d, ty]]`.
    pub matrix: [[f64; 3]; 2],
}

impl SimilarityTransform {
    /// Least-squares similarity transform from `source` to `destination`.
    ///
    /// Returns `None` for degenerate input (fewer than two points, non-finite
    /// values, or all source points coincident) instead of producing a garbage
    /// crop.
    pub fn estimate(source: &[[f32; 2]], destination: &[[f32; 2]]) -> Option<Self> {
        if source.len() != destination.len() || source.len() < 2 {
            return None;
        }
        if source
            .iter()
            .chain(destination.iter())
            .any(|point| !point[0].is_finite() || !point[1].is_finite())
        {
            return None;
        }

        let count = source.len() as f64;
        let mut source_mean = [0.0f64; 2];
        let mut destination_mean = [0.0f64; 2];
        for (from, to) in source.iter().zip(destination.iter()) {
            source_mean[0] += f64::from(from[0]) / count;
            source_mean[1] += f64::from(from[1]) / count;
            destination_mean[0] += f64::from(to[0]) / count;
            destination_mean[1] += f64::from(to[1]) / count;
        }

        let (mut a00, mut a01, mut a10, mut a11) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let mut variance = 0.0f64;
        for (from, to) in source.iter().zip(destination.iter()) {
            let sx = f64::from(from[0]) - source_mean[0];
            let sy = f64::from(from[1]) - source_mean[1];
            let dx = f64::from(to[0]) - destination_mean[0];
            let dy = f64::from(to[1]) - destination_mean[1];
            a00 += dx * sx;
            a01 += dx * sy;
            a10 += dy * sx;
            a11 += dy * sy;
            variance += sx * sx + sy * sy;
        }
        a00 /= count;
        a01 /= count;
        a10 /= count;
        a11 /= count;
        variance /= count;

        let magnitude = ((a00 + a11).powi(2) + (a10 - a01).powi(2)).sqrt();
        if variance <= f64::EPSILON || magnitude <= f64::EPSILON {
            return None;
        }
        let cos = (a00 + a11) / magnitude;
        let sin = (a10 - a01) / magnitude;
        let scale = magnitude / variance;

        let m00 = scale * cos;
        let m01 = -scale * sin;
        let m10 = scale * sin;
        let m11 = scale * cos;
        let tx = destination_mean[0] - (m00 * source_mean[0] + m01 * source_mean[1]);
        let ty = destination_mean[1] - (m10 * source_mean[0] + m11 * source_mean[1]);

        Some(Self {
            matrix: [[m00, m01, tx], [m10, m11, ty]],
        })
    }

    pub fn apply(&self, point: [f32; 2]) -> [f64; 2] {
        let x = f64::from(point[0]);
        let y = f64::from(point[1]);
        [
            self.matrix[0][0] * x + self.matrix[0][1] * y + self.matrix[0][2],
            self.matrix[1][0] * x + self.matrix[1][1] * y + self.matrix[1][2],
        ]
    }

    /// Inverse mapping used by the resampler, or `None` when the transform
    /// collapses the plane.
    pub fn inverse(&self) -> Option<[[f64; 3]; 2]> {
        let [[a, b, tx], [c, d, ty]] = self.matrix;
        let determinant = a * d - b * c;
        if determinant.abs() <= f64::EPSILON {
            return None;
        }
        let ia = d / determinant;
        let ib = -b / determinant;
        let ic = -c / determinant;
        let id = a / determinant;
        Some([
            [ia, ib, -(ia * tx + ib * ty)],
            [ic, id, -(ic * tx + id * ty)],
        ])
    }
}

/// Aligns `source` to a `size`x`size` crop using the canonical landmark
/// positions. Returns `None` when the landmarks cannot define a transform.
pub fn align_face(source: &RgbImage, landmarks: &[[f32; 2]], size: u32) -> Option<RgbImage> {
    if size == 0 {
        return None;
    }
    let scale = size as f32 / 112.0;
    let destination: Vec<[f32; 2]> = REFERENCE_LANDMARKS_112
        .iter()
        .map(|point| [point[0] * scale, point[1] * scale])
        .collect();
    let transform = SimilarityTransform::estimate(landmarks, &destination)?;
    let inverse = transform.inverse()?;

    let mut data = vec![0u8; size as usize * size as usize * 3];
    for y in 0..size {
        for x in 0..size {
            let source_x =
                inverse[0][0] * f64::from(x) + inverse[0][1] * f64::from(y) + inverse[0][2];
            let source_y =
                inverse[1][0] * f64::from(x) + inverse[1][1] * f64::from(y) + inverse[1][2];
            let index = (y as usize * size as usize + x as usize) * 3;
            for channel in 0..3 {
                data[index + channel] = sample_bilinear(source, source_x, source_y, channel as u8);
            }
        }
    }
    RgbImage::new(size, size, data).ok()
}

/// Bilinear sample with zero outside the image, matching
/// `warpAffine(..., BORDER_CONSTANT, 0)`.
fn sample_bilinear(source: &RgbImage, x: f64, y: f64, channel: u8) -> u8 {
    if !x.is_finite() || !y.is_finite() {
        return 0;
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let tap = |cx: f64, cy: f64| -> f64 {
        let cx = cx as i64;
        let cy = cy as i64;
        if cx < 0 || cy < 0 || cx >= source.width() as i64 || cy >= source.height() as i64 {
            return 0.0;
        }
        f64::from(source.pixel(cx as u32, cy as u32)[channel as usize])
    };
    let top = tap(x0, y0) * (1.0 - fx) + tap(x0 + 1.0, y0) * fx;
    let bottom = tap(x0, y0 + 1.0) * (1.0 - fx) + tap(x0 + 1.0, y0 + 1.0) * fx;
    (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> RgbImage {
        let mut data = Vec::with_capacity(width as usize * height as usize * 3);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[x as u8, y as u8, ((x + y) % 256) as u8]);
            }
        }
        RgbImage::new(width, height, data).unwrap()
    }

    #[test]
    fn identity_landmarks_reproduce_the_reference_positions() {
        let transform =
            SimilarityTransform::estimate(&REFERENCE_LANDMARKS_112, &REFERENCE_LANDMARKS_112)
                .unwrap();
        for point in REFERENCE_LANDMARKS_112 {
            let mapped = transform.apply(point);
            assert!((mapped[0] - f64::from(point[0])).abs() < 1e-6);
            assert!((mapped[1] - f64::from(point[1])).abs() < 1e-6);
        }
    }

    #[test]
    fn recovers_a_known_similarity_transform() {
        // dst = 2 * R(90deg) * src + (10, -5)
        let source: Vec<[f32; 2]> = vec![
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [5.0, 5.0],
        ];
        let destination: Vec<[f32; 2]> = source
            .iter()
            .map(|point| [10.0 - 2.0 * point[1], -5.0 + 2.0 * point[0]])
            .collect();
        let transform = SimilarityTransform::estimate(&source, &destination).unwrap();
        for (from, to) in source.iter().zip(destination.iter()) {
            let mapped = transform.apply(*from);
            assert!(
                (mapped[0] - f64::from(to[0])).abs() < 1e-4,
                "{mapped:?} != {to:?}"
            );
            assert!((mapped[1] - f64::from(to[1])).abs() < 1e-4);
        }
    }

    #[test]
    fn degenerate_input_is_rejected() {
        let identical = vec![[1.0f32, 1.0]; 5];
        assert!(SimilarityTransform::estimate(&identical, &REFERENCE_LANDMARKS_112).is_none());
        assert!(SimilarityTransform::estimate(&[[0.0, 0.0]], &[[1.0, 1.0]]).is_none());
        let mut not_finite = REFERENCE_LANDMARKS_112;
        not_finite[2][0] = f32::NAN;
        assert!(SimilarityTransform::estimate(&not_finite, &REFERENCE_LANDMARKS_112).is_none());
        assert!(align_face(&gradient(8, 8), &identical, 112).is_none());
    }

    #[test]
    fn aligning_reference_landmarks_keeps_the_center_pixel() {
        let source = gradient(160, 160);
        let aligned = align_face(&source, &REFERENCE_LANDMARKS_112, 112).unwrap();
        assert_eq!(aligned.width(), 112);
        assert_eq!(aligned.height(), 112);
        // The transform is the identity, so every pixel survives.
        for y in (0..112).step_by(17) {
            for x in (0..112).step_by(17) {
                assert_eq!(aligned.pixel(x, y), source.pixel(x, y));
            }
        }
    }

    #[test]
    fn alignment_scales_with_the_requested_size() {
        let source = gradient(160, 160);
        let small = align_face(&source, &REFERENCE_LANDMARKS_112, 56).unwrap();
        assert_eq!(small.width(), 56);
        let large = align_face(&source, &REFERENCE_LANDMARKS_112, 224).unwrap();
        assert_eq!(large.width(), 224);
    }

    #[test]
    fn translated_landmarks_shift_the_crop() {
        let source = gradient(160, 160);
        let shifted: Vec<[f32; 2]> = REFERENCE_LANDMARKS_112
            .iter()
            .map(|point| [point[0] + 10.0, point[1]])
            .collect();
        let aligned = align_face(&source, &shifted, 112).unwrap();
        // Pixel (0,0) of the crop now reads from source x=10.
        assert_eq!(aligned.pixel(0, 0), source.pixel(10, 0));
    }

    #[test]
    fn out_of_image_samples_are_zero() {
        let source = gradient(16, 16);
        let far: Vec<[f32; 2]> = REFERENCE_LANDMARKS_112
            .iter()
            .map(|point| [point[0] + 1000.0, point[1] + 1000.0])
            .collect();
        let aligned = align_face(&source, &far, 112).unwrap();
        assert_eq!(aligned.pixel(56, 56), [0, 0, 0]);
    }
}
