//! Host-side analyzer boundary. Model output is disposable; user data stays outside it.
use oxy_domain::{AnalyzerDescriptor, AnalyzerInputPolicy, AnalyzerRegion, FaceAnalyzerSettings};
use oxy_faces::{FaceAnalyzer, FaceModelPaths, RgbImage};
use oxy_runtime::CancellationToken;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum AnalyzerError {
    #[error("analysis cancelled")]
    Cancelled,
    #[error("invalid RGB handoff")]
    InvalidInput,
    #[error("analyzer failed: {0}")]
    Failed(String),
}

pub struct AnalyzeInput<'a> {
    pub asset_id: &'a str,
    pub source_revision: &'a str,
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [u8],
}

impl AnalyzeInput<'_> {
    pub fn validate(&self) -> Result<(), AnalyzerError> {
        let size = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|size| size.checked_mul(3));
        if self.asset_id.is_empty()
            || self.source_revision.is_empty()
            || self.width == 0
            || self.height == 0
            || size != Some(self.pixels.len())
        {
            return Err(AnalyzerError::InvalidInput);
        }
        Ok(())
    }
}

/// Feature vectors stay binary and never enter descriptor JSON or sidecars.
pub struct AnalyzeOutput {
    pub regions: Vec<AnalyzerRegion>,
    pub features: Vec<Vec<f32>>,
}

pub trait Analyzer: Send + Sync {
    fn descriptor(&self) -> &AnalyzerDescriptor;
    fn analyze(
        &self,
        input: &AnalyzeInput<'_>,
        cancel: &CancellationToken,
    ) -> Result<AnalyzeOutput, AnalyzerError>;
}

/// Reference adapter and explicit development fallback; not a plugin sandbox.
pub struct BuiltInFaceAnalyzer {
    inner: FaceAnalyzer,
    descriptor: AnalyzerDescriptor,
}

impl BuiltInFaceAnalyzer {
    pub fn load(
        models: &FaceModelPaths,
        settings: FaceAnalyzerSettings,
    ) -> Result<Self, AnalyzerError> {
        let inner = FaceAnalyzer::load(models, settings)
            .map_err(|error| AnalyzerError::Failed(error.to_string()))?;
        let descriptor = AnalyzerDescriptor {
            id: "faces".into(),
            version: "1".into(),
            input_policy: AnalyzerInputPolicy::DisplayOrientedRgb,
            analysis_fingerprint: inner.detector_fingerprint(),
            feature_fingerprint: inner.embedder_fingerprint().into(),
            feature_dimension: inner.embedding_dim(),
        };
        Ok(Self { inner, descriptor })
    }
}

impl Analyzer for BuiltInFaceAnalyzer {
    fn descriptor(&self) -> &AnalyzerDescriptor {
        &self.descriptor
    }

    fn analyze(
        &self,
        input: &AnalyzeInput<'_>,
        cancel: &CancellationToken,
    ) -> Result<AnalyzeOutput, AnalyzerError> {
        input.validate()?;
        if cancel.is_cancelled() {
            return Err(AnalyzerError::Cancelled);
        }
        let image = RgbImage::new(input.width, input.height, input.pixels.to_vec())
            .map_err(|_| AnalyzerError::InvalidInput)?;
        let faces = self
            .inner
            .analyze_cancellable(
                input.asset_id,
                Path::new(""),
                input.source_revision,
                &image,
                || cancel.is_cancelled(),
            )
            .map_err(|error| match error {
                oxy_faces::FaceError::Cancelled => AnalyzerError::Cancelled,
                other => AnalyzerError::Failed(other.to_string()),
            })?;
        let mut regions = Vec::with_capacity(faces.len());
        let mut features = Vec::with_capacity(faces.len());
        for face in faces {
            let observation = face.observation;
            regions.push(AnalyzerRegion {
                id: observation.observation_id,
                index: observation.local_index,
                rect: observation.bbox,
                landmarks: observation.landmarks,
                score: observation.detection_score,
            });
            features.push(face.embedding);
        }
        Ok(AnalyzeOutput { regions, features })
    }
}

mod process;
pub use process::{ProcessAnalyzer, serve_worker, verify_models, verify_pack};
