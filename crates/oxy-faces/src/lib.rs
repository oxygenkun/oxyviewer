//! Face detection, alignment, embedding, clustering, and person matching.
//!
//! This crate is the machine half of the people feature: it turns pixels into
//! [`oxy_domain::FaceObservation`]s and similarity scores. It owns no database,
//! no file writes, and no user decisions — a person and a confirmation belong
//! to the Host domain, so swapping the models here can never lose them.
//!
//! # Models
//!
//! Two ONNX models are used, both executed by the pure-Rust `tract` runtime so
//! no system OpenCV, CUDA, or Python installation is required:
//!
//! * SCRFD-10G KPS — face detection and five landmarks.
//! * AdaFace IR-101 — 512-dimensional identity embeddings.
//!
//! The postprocessing, alignment, and preprocessing mirror OpenCV's
//! `objdetect` implementation so results are reproducible against the
//! reference. See `tests/reference_parity.rs`.

mod adaface;
mod align;
mod analyzer;
mod calibration;
mod cluster;
mod detection;
mod image;
mod managed_models;
mod matcher;
mod scrfd;

pub use adaface::AdaFaceEmbedder;
pub use align::{REFERENCE_LANDMARKS_112, SimilarityTransform, align_face};
pub use analyzer::{AnalyzedFace, FaceAnalyzer, FaceModelPaths};
pub use calibration::{MIN_SAMPLES_PER_CLASS, populations_separate, recommend_threshold};
pub use cluster::{ClusterInput, cluster_embeddings, cluster_embeddings_cancellable};
pub use detection::DetectedFace;
pub use image::{ChannelOrder, Letterboxed, RgbImage, letterbox, resize_bilinear};
pub use managed_models::{
    ManagedModelManifest, ManagedModelSpec, managed_model_manifest, managed_model_paths,
};
pub use matcher::{
    PersonGallery, PersonMatch, match_person, match_person_cancellable, normalize_embedding,
    similarity, similarity_normalized,
};
pub use scrfd::ScrfdDetector;

use thiserror::Error;

/// Failures raised by the face analyzer. None of them are user-data failures:
/// a broken or missing model only invalidates rebuildable machine output.
#[derive(Debug, Error)]
pub enum FaceError {
    #[error("face analysis cancelled")]
    Cancelled,
    #[error("invalid RGB image: {width}x{height} with {bytes} bytes")]
    InvalidImage {
        width: u32,
        height: u32,
        bytes: usize,
    },
    #[error("failed to read model {path}: {source}")]
    ModelRead {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to load model {path}: {message}")]
    ModelLoad {
        path: std::path::PathBuf,
        message: String,
    },
    #[error("model {path} does not have the expected {expected} layout")]
    UnexpectedModel {
        path: std::path::PathBuf,
        expected: String,
    },
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("failed to read the source revision of {path}: {message}")]
    SourceRevision {
        path: std::path::PathBuf,
        message: String,
    },
    #[error("embedding for {observation_id} has {found} dimensions, expected {expected}")]
    UnexpectedEmbedding {
        observation_id: String,
        found: usize,
        expected: usize,
    },
    #[error(
        "clustering {count} faces exceeds the exact-comparison limit of {limit}; narrow the scope"
    )]
    ClusteringTooLarge { count: usize, limit: usize },
}

pub(crate) fn inference_error(error: impl std::fmt::Display) -> FaceError {
    FaceError::Inference(error.to_string())
}

pub(crate) fn check_cancelled(cancelled: &impl Fn() -> bool) -> Result<(), FaceError> {
    if cancelled() {
        Err(FaceError::Cancelled)
    } else {
        Ok(())
    }
}
