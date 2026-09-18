//! YuNet face detection.
//!
//! The graph emits raw per-anchor tensors for three strides; decoding, score
//! fusion, and non-maximum suppression happen here. The decode mirrors OpenCV's
//! `FaceDetectorYNImpl::postProcess` so detections can be compared against the
//! reference implementation.

use std::path::{Path, PathBuf};

use oxy_domain::FaceAnalyzerSettings;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use tract_onnx::prelude::*;
use tract_onnx::tract_core::internal::SimplePlan;

/// The concrete runnable plan type `into_runnable()` produces for an ONNX model.
type FaceRunnable = Arc<SimplePlan<TypedFact, Box<dyn TypedOp>>>;

use crate::image::{ChannelOrder, Letterboxed};
use crate::{FaceError, inference_error};

/// Anchor strides, smallest first. Must match the exported graph.
const STRIDES: [usize; 3] = [8, 16, 32];

/// One detection in letterbox pixel space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectedFace {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Five landmarks: right eye, left eye, nose tip, right mouth corner, left
    /// mouth corner.
    pub landmarks: [[f32; 2]; 5],
    pub score: f32,
}

/// A loaded YuNet detector.
pub struct YuNetDetector {
    model: FaceRunnable,
    input_size: u32,
    anchors: [usize; 3],
    fingerprint: String,
}

impl std::fmt::Debug for YuNetDetector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("YuNetDetector")
            .field("input_size", &self.input_size)
            .field("anchors", &self.anchors)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl YuNetDetector {
    /// Loads the model and records a fingerprint of the exact bytes and
    /// postprocessing version.
    ///
    /// `input_size` must equal the fixed spatial input of the exported graph.
    /// The anchor layout is validated by a probe inference, so a mismatched
    /// model fails here instead of producing silently wrong boxes.
    pub fn load(path: &Path, input_size: u32) -> Result<Self, FaceError> {
        let bytes = std::fs::read(path).map_err(|source| FaceError::ModelRead {
            path: path.to_path_buf(),
            source,
        })?;
        let digest = hex_digest(&bytes);
        let model: FaceRunnable = tract_onnx::onnx()
            .model_for_path(path)
            .and_then(InferenceModelExt::into_optimized)
            .and_then(IntoRunnable::into_runnable)
            .map_err(|error| FaceError::ModelLoad {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;

        let anchors = STRIDES.map(|stride| (input_size as usize / stride).pow(2));
        let detector = Self {
            model,
            input_size,
            anchors,
            fingerprint: format!("yunet/{}/detect-v1", &digest[..16]),
        };
        detector.validate_layout(path)?;
        Ok(detector)
    }

    /// Fingerprint of the detector bytes and detection postprocessing. Changing
    /// it invalidates detections and everything downstream, never user data.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn input_size(&self) -> u32 {
        self.input_size
    }

    /// Runs detection on an already letterboxed image. Results are in letterbox
    /// pixel space; callers map them back with [`Letterboxed::to_source_rect`].
    pub fn detect(
        &self,
        boxed: &Letterboxed,
        settings: &FaceAnalyzerSettings,
    ) -> Result<Vec<DetectedFace>, FaceError> {
        if boxed.image.width() != self.input_size || boxed.image.height() != self.input_size {
            return Err(FaceError::UnexpectedModel {
                path: PathBuf::from("<letterbox>"),
                expected: format!("{}x{} input", self.input_size, self.input_size),
            });
        }

        let tensor = boxed.image.to_nchw_f32(ChannelOrder::Bgr);
        let input = tract_ndarray::Array4::<f32>::from_shape_vec(
            (1, 3, self.input_size as usize, self.input_size as usize),
            tensor,
        )
        .map_err(inference_error)?;
        let outputs = self
            .model
            .run(tvec!(input.into_tensor().into()))
            .map_err(inference_error)?;
        self.decode(&outputs, settings)
    }

    fn decode(
        &self,
        outputs: &[TValue],
        settings: &FaceAnalyzerSettings,
    ) -> Result<Vec<DetectedFace>, FaceError> {
        if outputs.len() < STRIDES.len() * 4 {
            return Err(FaceError::UnexpectedModel {
                path: PathBuf::from("<yunet>"),
                expected: format!("{} score/box/keypoint outputs", STRIDES.len() * 4),
            });
        }

        let mut candidates: Vec<DetectedFace> = Vec::new();
        for (level, stride) in STRIDES.iter().enumerate() {
            let expected = self.anchors[level];
            let classification = tensor_rows(&outputs[level], expected, 1)?;
            let objectness = tensor_rows(&outputs[level + STRIDES.len()], expected, 1)?;
            let boxes = tensor_rows(&outputs[level + STRIDES.len() * 2], expected, 4)?;
            let keypoints = tensor_rows(&outputs[level + STRIDES.len() * 3], expected, 10)?;

            let columns = self.input_size as usize / stride;
            for index in 0..expected {
                let cls = classification[index].clamp(0.0, 1.0);
                let obj = objectness[index].clamp(0.0, 1.0);
                let score = (cls * obj).sqrt();
                if score < settings.detection_confidence {
                    continue;
                }
                let row = index / columns;
                let column = index % columns;
                let stride = *stride as f32;
                let center_x = (column as f32 + boxes[index * 4]) * stride;
                let center_y = (row as f32 + boxes[index * 4 + 1]) * stride;
                let width = boxes[index * 4 + 2].exp() * stride;
                let height = boxes[index * 4 + 3].exp() * stride;

                let mut landmarks = [[0.0f32; 2]; 5];
                for (point, landmark) in landmarks.iter_mut().enumerate() {
                    landmark[0] = (keypoints[index * 10 + point * 2] + column as f32) * stride;
                    landmark[1] = (keypoints[index * 10 + point * 2 + 1] + row as f32) * stride;
                }

                if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
                    continue;
                }
                candidates.push(DetectedFace {
                    x: center_x - width / 2.0,
                    y: center_y - height / 2.0,
                    width,
                    height,
                    landmarks,
                    score,
                });
            }
        }

        let mut kept = non_maximum_suppression(
            candidates,
            settings.nms_threshold,
            settings.max_faces_per_asset as usize,
        );
        // Small faces cannot produce a usable embedding, so they are dropped
        // before they ever reach the review queue.
        kept.retain(|face| face.width.min(face.height) >= settings.min_face_pixels as f32);
        Ok(kept)
    }

    /// Probe inference: the graph must produce exactly three anchors-per-level
    /// groups whose sizes match `input_size / stride`.
    fn validate_layout(&self, path: &Path) -> Result<(), FaceError> {
        let expected = |message: String| FaceError::UnexpectedModel {
            path: path.to_path_buf(),
            expected: message,
        };
        let tensor = vec![0.0f32; 3 * self.input_size as usize * self.input_size as usize];
        let input = tract_ndarray::Array4::<f32>::from_shape_vec(
            (1, 3, self.input_size as usize, self.input_size as usize),
            tensor,
        )
        .map_err(inference_error)?;
        let outputs = self
            .model
            .run(tvec!(input.into_tensor().into()))
            .map_err(inference_error)?;
        if outputs.len() != STRIDES.len() * 4 {
            return Err(expected(format!(
                "{} outputs, found {}",
                STRIDES.len() * 4,
                outputs.len()
            )));
        }
        for (level, anchors) in self.anchors.iter().enumerate() {
            let shapes = [
                outputs[level].shape(),
                outputs[level + STRIDES.len()].shape(),
                outputs[level + STRIDES.len() * 2].shape(),
                outputs[level + STRIDES.len() * 3].shape(),
            ];
            let widths = [1usize, 1, 4, 10];
            for (shape, width) in shapes.iter().zip(widths) {
                if shape.len() != 3 || shape[1] != *anchors || shape[2] != width {
                    return Err(expected(format!(
                        "stride {} level with {anchors} anchors of width {width}, found {shape:?}",
                        STRIDES[level]
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Flattens one `1xNxC` output into a contiguous `Vec<f32>` of `N*C` values.
fn tensor_rows(value: &TValue, anchors: usize, width: usize) -> Result<Vec<f32>, FaceError> {
    let view = value
        .to_plain_array_view::<f32>()
        .map_err(inference_error)?;
    let expected = [1usize, anchors, width];
    if view.shape() != expected {
        return Err(FaceError::UnexpectedModel {
            path: PathBuf::from("<yunet>"),
            expected: format!("{expected:?}, found {:?}", view.shape()),
        });
    }
    Ok(view.iter().copied().collect())
}

/// Greedy non-maximum suppression over score order, matching OpenCV's
/// `NMSBoxes` shape: at most `top_k` candidates are considered and a box is
/// suppressed when its overlap is strictly greater than `threshold`.
fn non_maximum_suppression(
    mut faces: Vec<DetectedFace>,
    threshold: f32,
    top_k: usize,
) -> Vec<DetectedFace> {
    faces.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let limit = top_k.max(1).min(faces.len());
    let mut suppressed = vec![false; limit];
    let mut kept = Vec::new();
    for index in 0..limit {
        if suppressed[index] {
            continue;
        }
        kept.push(faces[index]);
        for candidate in index + 1..limit {
            if suppressed[candidate] {
                continue;
            }
            if box_iou(&faces[index], &faces[candidate]) > threshold {
                suppressed[candidate] = true;
            }
        }
    }
    kept
}

fn box_iou(left: &DetectedFace, right: &DetectedFace) -> f32 {
    let x1 = left.x.max(right.x);
    let y1 = left.y.max(right.y);
    let x2 = (left.x + left.width).min(right.x + right.width);
    let y2 = (left.y + left.height).min(right.y + right.height);
    let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = left.width * left.height + right.width * right.height - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f32, y: f32, size: f32, score: f32) -> DetectedFace {
        DetectedFace {
            x,
            y,
            width: size,
            height: size,
            landmarks: [[0.0; 2]; 5],
            score,
        }
    }

    #[test]
    fn nms_keeps_the_highest_scoring_overlapping_box() {
        let kept = non_maximum_suppression(
            vec![
                face(0.0, 0.0, 10.0, 0.7),
                face(1.0, 1.0, 10.0, 0.95),
                face(100.0, 100.0, 10.0, 0.8),
            ],
            0.3,
            64,
        );
        assert_eq!(kept.len(), 2);
        assert!((kept[0].score - 0.95).abs() < 1e-6);
        assert!((kept[1].score - 0.8).abs() < 1e-6);
    }

    #[test]
    fn nms_respects_top_k_before_suppression() {
        let kept = non_maximum_suppression(
            vec![
                face(0.0, 0.0, 10.0, 0.2),
                face(50.0, 50.0, 10.0, 0.5),
                face(100.0, 100.0, 10.0, 0.9),
            ],
            0.3,
            1,
        );
        assert_eq!(kept.len(), 1);
        assert!((kept[0].score - 0.9).abs() < 1e-6);
    }

    #[test]
    fn disjoint_boxes_are_never_suppressed() {
        let kept = non_maximum_suppression(
            vec![face(0.0, 0.0, 10.0, 0.9), face(20.0, 0.0, 10.0, 0.8)],
            0.3,
            64,
        );
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn digest_is_stable_and_hex() {
        let digest = hex_digest(b"yunet");
        assert_eq!(digest.len(), 64);
        assert_eq!(digest, hex_digest(b"yunet"));
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
