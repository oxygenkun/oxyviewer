//! SCRFD-10G KPS face detection.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxy_domain::FaceAnalyzerSettings;
use sha2::{Digest, Sha256};
use tract_onnx::prelude::*;
use tract_onnx::tract_core::internal::SimplePlan;

use crate::detection::{DetectedFace, non_maximum_suppression};
use crate::image::{ChannelOrder, Letterboxed};
use crate::{FaceError, inference_error};

type FaceRunnable = Arc<SimplePlan<TypedFact, Box<dyn TypedOp>>>;
const STRIDES: [usize; 3] = [8, 16, 32];
const ANCHORS_PER_LOCATION: usize = 2;

pub struct ScrfdDetector {
    model: FaceRunnable,
    input_size: u32,
    fingerprint: String,
}

impl std::fmt::Debug for ScrfdDetector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScrfdDetector")
            .field("input_size", &self.input_size)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl ScrfdDetector {
    pub fn load(path: &Path, input_size: u32) -> Result<Self, FaceError> {
        let digest = file_digest(path)?;
        let size = input_size as usize;
        let model: FaceRunnable = tract_onnx::onnx()
            .model_for_path(path)
            .and_then(|model| model.with_input_fact(0, f32::fact([1, 3, size, size]).into()))
            .and_then(InferenceModelExt::into_optimized)
            .and_then(IntoRunnable::into_runnable)
            .map_err(|error| FaceError::ModelLoad {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        let detector = Self {
            model,
            input_size,
            fingerprint: format!("scrfd-10g-kps/{}/detect-v1", &digest[..16]),
        };
        Ok(detector)
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    pub fn input_size(&self) -> u32 {
        self.input_size
    }

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
        let mut tensor = boxed.image.to_nchw_f32(ChannelOrder::Rgb);
        for value in &mut tensor {
            *value = (*value - 127.5) / 128.0;
        }
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
        if outputs.len() != 9 {
            return Err(layout_error(format!("9 outputs, found {}", outputs.len())));
        }
        let mut candidates = Vec::new();
        for (level, stride) in STRIDES.iter().copied().enumerate() {
            let columns = self.input_size as usize / stride;
            let anchors = columns * columns * ANCHORS_PER_LOCATION;
            let scores = tensor_values(&outputs[level], anchors, 1)?;
            let boxes = tensor_values(&outputs[level + 3], anchors, 4)?;
            let keypoints = tensor_values(&outputs[level + 6], anchors, 10)?;
            for index in 0..anchors {
                let score = scores[index];
                if score < settings.detection_confidence {
                    continue;
                }
                let location = index / ANCHORS_PER_LOCATION;
                let center_x = (location % columns) as f32 * stride as f32;
                let center_y = (location / columns) as f32 * stride as f32;
                let offset = index * 4;
                let left = boxes[offset] * stride as f32;
                let top = boxes[offset + 1] * stride as f32;
                let right = boxes[offset + 2] * stride as f32;
                let bottom = boxes[offset + 3] * stride as f32;
                let (width, height) = (left + right, top + bottom);
                if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
                    continue;
                }
                let mut landmarks = [[0.0; 2]; 5];
                for (point, landmark) in landmarks.iter_mut().enumerate() {
                    landmark[0] = center_x + keypoints[index * 10 + point * 2] * stride as f32;
                    landmark[1] = center_y + keypoints[index * 10 + point * 2 + 1] * stride as f32;
                }
                candidates.push(DetectedFace {
                    x: center_x - left,
                    y: center_y - top,
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
        kept.retain(|face| face.width.min(face.height) >= settings.min_face_pixels as f32);
        Ok(kept)
    }
}

fn tensor_values(value: &TValue, rows: usize, width: usize) -> Result<Vec<f32>, FaceError> {
    let view = value
        .to_plain_array_view::<f32>()
        .map_err(inference_error)?;
    let shape = view.shape();
    if shape != [rows, width] && shape != [1, rows, width] {
        return Err(layout_error(format!(
            "{rows} rows of width {width}, found {shape:?}"
        )));
    }
    Ok(view.iter().copied().collect())
}

fn layout_error(expected: impl Into<String>) -> FaceError {
    FaceError::UnexpectedModel {
        path: PathBuf::from("<scrfd>"),
        expected: expected.into(),
    }
}

fn file_digest(path: &Path) -> Result<String, FaceError> {
    let bytes = std::fs::read(path).map_err(|source| FaceError::ModelRead {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_batched_and_unbatched_output_shapes() {
        let batched = tract_ndarray::Array3::<f32>::zeros((1, 2, 1)).into_tensor();
        let unbatched = tract_ndarray::Array2::<f32>::zeros((2, 1)).into_tensor();
        assert_eq!(tensor_values(&batched.into(), 2, 1).unwrap().len(), 2);
        assert_eq!(tensor_values(&unbatched.into(), 2, 1).unwrap().len(), 2);
    }
}
