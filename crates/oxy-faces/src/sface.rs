//! SFace embedding extraction.
//!
//! `FaceRecognizerSF::feature` feeds a 112x112 aligned crop as an RGB float
//! tensor scaled to `0..=255` with no mean subtraction; this module reproduces
//! exactly that. Similarity is only defined after L2 normalization, which
//! [`crate::similarity`] applies to both sides.

use std::path::{Path, PathBuf};

use std::sync::Arc;

use tract_onnx::prelude::*;
use tract_onnx::tract_core::internal::SimplePlan;

/// The concrete runnable plan type `into_runnable()` produces for an ONNX model.
type FaceRunnable = Arc<SimplePlan<TypedFact, Box<dyn TypedOp>>>;

use crate::image::{ChannelOrder, RgbImage};
use crate::yunet::hex_digest;
use crate::{FaceError, inference_error};

/// The input size SFace was trained on.
pub const ALIGNED_SIZE: u32 = 112;

/// The embedding width SFace produces.
pub const EMBEDDING_DIM: usize = 128;

/// A loaded SFace embedder.
pub struct SFaceEmbedder {
    model: FaceRunnable,
    fingerprint: String,
}

impl std::fmt::Debug for SFaceEmbedder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SFaceEmbedder")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl SFaceEmbedder {
    pub fn load(path: &Path) -> Result<Self, FaceError> {
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
        let embedder = Self {
            model,
            fingerprint: format!("sface/{}/align-v1/embed-v1", &digest[..16]),
        };
        embedder.validate_layout(path)?;
        Ok(embedder)
    }

    /// Fingerprint of the embedder bytes, alignment version, and preprocessing.
    /// Changing it invalidates embeddings, clusters, and candidates — never a
    /// person or a confirmation.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Extracts the raw (unnormalized) embedding of an aligned crop.
    pub fn embed(&self, aligned: &RgbImage) -> Result<Vec<f32>, FaceError> {
        if aligned.width() != ALIGNED_SIZE || aligned.height() != ALIGNED_SIZE {
            return Err(FaceError::UnexpectedModel {
                path: PathBuf::from("<aligned crop>"),
                expected: format!("{ALIGNED_SIZE}x{ALIGNED_SIZE} input"),
            });
        }
        let tensor = aligned.to_nchw_f32(ChannelOrder::Rgb);
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
            path: PathBuf::from("<sface>"),
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
        let tensor = vec![0.0f32; 3 * ALIGNED_SIZE as usize * ALIGNED_SIZE as usize];
        let input = tract_ndarray::Array4::<f32>::from_shape_vec(
            (1, 3, ALIGNED_SIZE as usize, ALIGNED_SIZE as usize),
            tensor,
        )
        .map_err(inference_error)?;
        let outputs = self
            .model
            .run(tvec!(input.into_tensor().into()))
            .map_err(inference_error)?;
        let shape = outputs.first().map(|output| output.shape().to_vec());
        if shape.as_deref() != Some(&[1, EMBEDDING_DIM][..]) {
            return Err(FaceError::UnexpectedModel {
                path: path.to_path_buf(),
                expected: format!("one [1, {EMBEDDING_DIM}] embedding output, found {shape:?}"),
            });
        }
        Ok(())
    }
}
