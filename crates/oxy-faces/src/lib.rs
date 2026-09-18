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
//! * YuNet — face detection (`FaceDetectorYN`-compatible postprocessing).
//! * SFace — 128-dimensional embeddings over a 5-landmark aligned crop.
//!
//! The postprocessing, alignment, and preprocessing mirror OpenCV's
//! `objdetect` implementation so results are reproducible against the
//! reference. See `tests/reference_parity.rs`.

mod align;
mod analyzer;
mod calibration;
mod cluster;
mod image;
mod matcher;
mod sface;
mod yunet;

pub use align::{REFERENCE_LANDMARKS_112, SimilarityTransform, align_face};
pub use analyzer::{AnalyzedFace, FaceAnalyzer, FaceModelPaths};
pub use calibration::{MIN_SAMPLES_PER_CLASS, populations_separate, recommend_threshold};
pub use cluster::{ClusterInput, cluster_embeddings};
pub use image::{ChannelOrder, Letterboxed, RgbImage, letterbox, resize_bilinear};
pub use matcher::{
    PersonGallery, PersonMatch, match_person, normalize_embedding, similarity,
    similarity_normalized,
};
pub use sface::SFaceEmbedder;
pub use yunet::{DetectedFace, YuNetDetector};

use thiserror::Error;

/// Failures raised by the face analyzer. None of them are user-data failures:
/// a broken or missing model only invalidates rebuildable machine output.
#[derive(Debug, Error)]
pub enum FaceError {
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
