//! Analysis pipeline, evidence and review decision contracts.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PipelineManifest {
    pub pipeline_id: String,
    pub pipeline_version: String,
    pub stages: Vec<PersonStageManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PersonStageManifest {
    pub stage_id: String,
    pub kind: PersonStageKind,
    pub depends_on: Vec<String>,
    pub input_schema: String,
    pub output_schema: String,
    pub runtime_version: String,
    pub preprocessing_version: String,
    pub postprocessing_version: String,
    pub feature_space_id: Option<String>,
    pub distance_metric: Option<PersonDistanceMetric>,
    pub calibration_id: Option<String>,
    pub artifact: Option<PersonModelArtifact>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum PersonStageKind {
    FaceDetector,
    BodyDetector,
    Association,
    QualityGate,
    FaceEncoder,
    BodyEncoder,
    Clusterer,
    Retriever,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum PersonDistanceMetric {
    Cosine,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PersonModelArtifact {
    pub artifact_id: String,
    pub download_url: String,
    pub format: PersonModelArtifactFormat,
    pub size_bytes: u64,
    pub sha256: String,
    pub license_name: String,
    pub license_url: String,
    pub license_status: PersonModelLicenseStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum PersonModelArtifactFormat {
    Onnx,
    Safetensors,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum PersonModelLicenseStatus {
    VerifiedForDistribution,
    ResearchOnly,
    Unresolved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum PersonAnalysisState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PersonAnalysisRun {
    pub run_id: String,
    pub folder_path: PathBuf,
    pub pipeline_id: String,
    pub pipeline_fingerprint: String,
    pub generation: i64,
    pub state: PersonAnalysisState,
    pub enumeration_complete: bool,
    pub total_tasks: u64,
    pub completed_tasks: u64,
    pub failed_tasks: u64,
}

/// Snapshot of an explicitly started background operation. Progress polling
/// reads this small snapshot rather than hashing model files or scanning images.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PersonOperationStatus {
    pub operation_id: String,
    pub folder_path: Option<PathBuf>,
    pub state: PersonOperationState,
    pub completed: u64,
    pub total: u64,
    pub detail: String,
    pub error: Option<String>,
    pub run: Option<PersonAnalysisRun>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum PersonOperationState {
    Downloading,
    Preparing,
    Analysing,
    Clustering,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PersonModelStatus {
    pub id: String,
    pub name: String,
    pub size_bytes: u64,
    pub installed: bool,
    pub download_available: bool,
    pub source_url: String,
    pub usage: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct BeginPersonAnalysis {
    pub folder_path: PathBuf,
    pub pipeline_id: String,
    pub pipeline_fingerprint: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PersonAnalysisTaskInput {
    pub asset_path: PathBuf,
    pub source_revision: String,
    pub stage_id: String,
    pub stage_fingerprint: String,
    pub stage_order: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PersonAnalysisTask {
    pub run_id: String,
    pub folder_path: PathBuf,
    pub asset_path: PathBuf,
    pub source_revision: String,
    pub stage_id: String,
    pub stage_fingerprint: String,
    pub stage_order: u32,
    /// Opaque token for this claim attempt. A recovered task gets a new token.
    pub claim_token: String,
}

/// Rebuildable detector evidence for one image instance. It cannot establish
/// a person's identity or override an explicit review decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DetectedPersonInstance {
    /// Content-derived cache ID, independent of detector output order.
    pub instance_id: String,
    /// Normalized source-image x, y, width, height.
    pub face_box: Option<[f64; 4]>,
    /// Five detector landmarks normalized by analysis-image width and height.
    /// Values may extend outside 0..1 when the detector predicts past an edge.
    pub face_landmarks: Option<[[f32; 2]; 5]>,
    pub body_box: Option<[f64; 4]>,
    pub face_score: Option<f32>,
    pub body_score: Option<f32>,
    pub association_score: Option<f32>,
}

/// Which encoder produced a cached feature vector.
///
/// The stored spelling lives here rather than in the crate that reads the
/// vector, so the feature-space contract in `person_feature_spaces` and the
/// code that validates a vector against it cannot disagree about what "face"
/// means. Face and body vectors are never interchangeable, even at equal
/// dimension, so this is part of the feature-space identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureModality {
    Face,
    Body,
}

impl FeatureModality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Face => "face",
            Self::Body => "body",
        }
    }

    pub fn from_text(value: &str) -> Option<Self> {
        match value {
            "face" => Some(Self::Face),
            "body" => Some(Self::Body),
            _ => None,
        }
    }
}

/// One cached vector that scored above the query threshold.
///
/// It carries the row identity, the instance it belongs to, and the similarity
/// the store computed, so the caller can page and de-duplicate without seeing
/// the vector itself.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonFeatureMatch {
    pub feature_row_id: i64,
    pub asset_path: PathBuf,
    pub instance_id: String,
    pub source_revision: String,
    pub pipeline_fingerprint: String,
    pub similarity: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum PersonReviewDecision {
    Pending,
    Belongs,
    DoesNotBelong,
    Deferred,
}

impl PersonReviewDecision {
    /// The spelling stored in the database and sent across IPC.
    ///
    /// It matches the serde name, so the column and the wire format cannot
    /// drift apart.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Belongs => "belongs",
            Self::DoesNotBelong => "doesNotBelong",
            Self::Deferred => "deferred",
        }
    }

    /// Reads [`Self::as_str`] back. An unknown word is [`Self::Pending`],
    /// because a decision the app cannot interpret has to be looked at again.
    pub fn from_text(value: &str) -> Self {
        match value {
            "belongs" => Self::Belongs,
            "doesNotBelong" => Self::DoesNotBelong,
            "deferred" => Self::Deferred,
            _ => Self::Pending,
        }
    }
}
