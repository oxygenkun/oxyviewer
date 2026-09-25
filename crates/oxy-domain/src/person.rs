use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderPerson {
    pub id: String,
    pub folder_path: PathBuf,
    pub display_name: Option<String>,
    pub identity_confirmed: bool,
    pub revision: i64,
    pub reference_instance_id: Option<String>,
    pub pending_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalPerson {
    pub id: String,
    pub display_name: String,
    pub reference_asset_path: PathBuf,
    pub reference_source_revision: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkHistoricalPerson {
    pub folder_path: PathBuf,
    pub subject_id: String,
    /// None creates a new historical identity from the confirmed folder person.
    pub historical_person_id: Option<String>,
    pub expected_revision: i64,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlinkHistoricalPerson {
    pub folder_path: PathBuf,
    pub subject_id: String,
    pub expected_revision: i64,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersonTagLink {
    pub historical_person_id: String,
    pub tag_id: Option<i64>,
    pub enabled: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPersonTagLink {
    pub folder_path: PathBuf,
    pub subject_id: String,
    pub tag_id: Option<i64>,
    pub enabled: bool,
    pub expected_revision: i64,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersonTagOverride {
    pub historical_person_id: String,
    pub asset_path: PathBuf,
    pub suppressed: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPersonTagOverride {
    pub folder_path: PathBuf,
    pub subject_id: String,
    pub asset_path: PathBuf,
    pub suppressed: bool,
    pub expected_revision: i64,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PipelineManifest {
    pub pipeline_id: String,
    pub pipeline_version: String,
    pub stages: Vec<PersonStageManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
#[serde(rename_all = "camelCase")]
pub enum PersonDistanceMetric {
    Cosine,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
#[serde(rename_all = "camelCase")]
pub enum PersonModelArtifactFormat {
    Onnx,
    Safetensors,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PersonModelLicenseStatus {
    VerifiedForDistribution,
    ResearchOnly,
    Unresolved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PersonAnalysisState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginPersonAnalysis {
    pub folder_path: PathBuf,
    pub pipeline_id: String,
    pub pipeline_fingerprint: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersonAnalysisTaskInput {
    pub asset_path: PathBuf,
    pub source_revision: String,
    pub stage_id: String,
    pub stage_fingerprint: String,
    pub stage_order: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonFilter {
    pub subject_id: Option<String>,
    pub state: PersonFilterState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PersonFilterState {
    Pending,
    Belongs,
    DoesNotBelong,
    Deferred,
    All,
    Unassigned,
    NeedsReview,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePersonInstance {
    pub folder_path: PathBuf,
    pub instance_id: String,
    pub source_revision: String,
    pub face_box: Option<[f64; 4]>,
    pub body_box: Option<[f64; 4]>,
    pub expected_revision: i64,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetFolderPerson {
    pub folder_path: PathBuf,
    pub subject_id: String,
    pub display_name: String,
    pub expected_revision: i64,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManualPersonAnchor {
    pub instance_id: String,
    pub source_identity_revision: Option<String>,
    pub face_box: Option<[f64; 4]>,
    pub body_box: Option<[f64; 4]>,
    pub needs_review: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonInstance {
    pub id: String,
    pub folder_path: PathBuf,
    pub asset_path: PathBuf,
    pub source_revision: String,
    /// Normalized image coordinates [left, top, width, height].
    pub face_box: Option<[f64; 4]>,
    pub body_box: Option<[f64; 4]>,
    pub needs_review: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonReview {
    pub instance: PersonInstance,
    pub subject_id: String,
    pub decision: PersonReviewDecision,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePersonInstance {
    pub folder_path: PathBuf,
    pub asset_path: PathBuf,
    pub source_revision: String,
    pub face_box: Option<[f64; 4]>,
    pub body_box: Option<[f64; 4]>,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPersonReview {
    pub folder_path: PathBuf,
    pub instance_id: String,
    pub subject_id: String,
    pub decision: PersonReviewDecision,
    pub expected_revision: i64,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmFolderPerson {
    pub folder_path: PathBuf,
    pub subject_id: String,
    pub reference_instance_id: String,
    pub display_name: String,
    pub expected_revision: i64,
    pub request_id: String,
}
