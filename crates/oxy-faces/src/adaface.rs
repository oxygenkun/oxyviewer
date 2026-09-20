//! AdaFace IR-101 embedding extraction.
//!
//! The exported model consumes a 112x112 BGR tensor normalized to `-1..=1`
//! and produces a 512-dimensional identity feature. The analyzer applies the
//! standard ArcFace five-landmark alignment before calling this module, then
//! L2-normalizes the returned feature at the shared boundary.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tract_onnx::prelude::*;
use tract_onnx::tract_core::internal::SimplePlan;

use crate::image::RgbImage;
use crate::{FaceError, inference_error};

type FaceRunnable = Arc<SimplePlan<TypedFact, Box<dyn TypedOp>>>;

pub const ALIGNED_SIZE: u32 = 112;
pub const EMBEDDING_DIM: usize = 512;

pub struct AdaFaceEmbedder {
    model: FaceRunnable,
    fingerprint: String,
}

impl std::fmt::Debug for AdaFaceEmbedder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdaFaceEmbedder")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl AdaFaceEmbedder {
    pub fn load(path: &Path) -> Result<Self, FaceError> {
        let digest = file_hex_digest(path)?;
        let model: FaceRunnable = tract_onnx::onnx()
            .model_for_path(path)
            .and_then(InferenceModelExt::into_optimized)
            .and_then(IntoRunnable::into_runnable)
            .map_err(|error| FaceError::ModelLoad {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        let embedder = Self {
            model,
            fingerprint: format!(
                "adaface-ir101/{}/arcface-align-v1/bgr-norm-v1",
                &digest[..16]
            ),
        };
        embedder.validate_layout(path)?;
        Ok(embedder)
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn embed(&self, aligned: &RgbImage) -> Result<Vec<f32>, FaceError> {
        if aligned.width() != ALIGNED_SIZE || aligned.height() != ALIGNED_SIZE {
            return Err(FaceError::UnexpectedModel {
                path: PathBuf::from("<aligned crop>"),
                expected: format!("{ALIGNED_SIZE}x{ALIGNED_SIZE} input"),
            });
        }
        let tensor = adaface_tensor(aligned);
        let input = tract_ndarray::Array4::<f32>::from_shape_vec(
            (1, 3, ALIGNED_SIZE as usize, ALIGNED_SIZE as usize),
            tensor,
        )
        .map_err(inference_error)?;
        let outputs = self
            .model
            .run(tvec!(input.into_tensor().into()))
            .map_err(inference_error)?;
        let output = outputs.first().ok_or_else(|| FaceError::UnexpectedModel {
            path: PathBuf::from("<adaface>"),
            expected: "one embedding output".into(),
        })?;
        let view = output
            .to_plain_array_view::<f32>()
            .map_err(inference_error)?;
        if view.len() != EMBEDDING_DIM {
            return Err(FaceError::UnexpectedEmbedding {
                observation_id: "<aligned crop>".into(),
                found: view.len(),
                expected: EMBEDDING_DIM,
            });
        }
        Ok(view.iter().copied().collect())
    }

    fn validate_layout(&self, path: &Path) -> Result<(), FaceError> {
        let image = RgbImage::new(
            ALIGNED_SIZE,
            ALIGNED_SIZE,
            vec![127; ALIGNED_SIZE as usize * ALIGNED_SIZE as usize * 3],
        )?;
        self.embed(&image).map(|_| ()).map_err(|error| match error {
            FaceError::UnexpectedEmbedding {
                found, expected, ..
            } => FaceError::UnexpectedModel {
                path: path.to_path_buf(),
                expected: format!("one [1, {expected}] embedding output, found {found} values"),
            },
            other => other,
        })
    }
}

fn file_hex_digest(path: &Path) -> Result<String, FaceError> {
    let mut file = std::fs::File::open(path).map_err(|source| FaceError::ModelRead {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|source| FaceError::ModelRead {
                path: path.to_path_buf(),
                source,
            })?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn adaface_tensor(image: &RgbImage) -> Vec<f32> {
    let plane = image.width() as usize * image.height() as usize;
    let mut tensor = vec![0.0; plane * 3];
    for (index, pixel) in image.data().chunks_exact(3).enumerate() {
        let normalized = |value: u8| (f32::from(value) / 255.0 - 0.5) / 0.5;
        tensor[index] = normalized(pixel[2]);
        tensor[plane + index] = normalized(pixel[1]);
        tensor[plane * 2 + index] = normalized(pixel[0]);
    }
    tensor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocessing_is_bgr_and_normalized() {
        let image = RgbImage::new(1, 1, vec![255, 127, 0]).unwrap();
        let tensor = adaface_tensor(&image);
        assert_eq!(tensor[0], -1.0);
        assert!((tensor[1] - (127.0 / 127.5 - 1.0)).abs() < 1e-6);
        assert_eq!(tensor[2], 1.0);
    }

    #[test]
    #[ignore = "requires the optional 249 MB AdaFace experiment model"]
    fn local_ir101_model_loads_and_outputs_512_values() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/native/face-models/adaface_ir_101.onnx");
        let embedder = AdaFaceEmbedder::load(&path).expect("AdaFace model loads");
        let image = RgbImage::new(
            ALIGNED_SIZE,
            ALIGNED_SIZE,
            vec![127; ALIGNED_SIZE as usize * ALIGNED_SIZE as usize * 3],
        )
        .unwrap();
        let embedding = embedder.embed(&image).expect("AdaFace inference succeeds");
        assert_eq!(embedding.len(), EMBEDDING_DIM);
        assert!(embedding.iter().all(|value| value.is_finite()));
    }
}
