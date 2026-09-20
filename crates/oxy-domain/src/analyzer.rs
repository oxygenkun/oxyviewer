use serde::{Deserialize, Serialize};

/// Host-owned rendering policy. An analyzer receives pixels, never a source path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnalyzerInputPolicy {
    DisplayOrientedRgb,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerDescriptor {
    pub id: String,
    pub version: String,
    pub input_policy: AnalyzerInputPolicy,
    pub analysis_fingerprint: String,
    pub feature_fingerprint: String,
    pub feature_dimension: usize,
}

/// Shared geometric result. Source paths and user facts are supplied only by the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerRegion {
    pub id: String,
    pub index: u32,
    pub rect: crate::NormalizedRect,
    pub landmarks: Vec<crate::NormalizedPoint>,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerPackManifest {
    pub id: String,
    pub version: String,
    pub host_api: u32,
    pub target_os: String,
    pub target_arch: String,
    pub entrypoint: String,
    pub entrypoint_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerWorkerInit {
    pub host_api: u32,
    pub model_directory: std::path::PathBuf,
    pub settings: crate::FaceAnalyzerSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerWorkerRequest {
    pub request_id: u64,
    pub asset_id: String,
    pub source_revision: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerWorkerReply {
    pub request_id: u64,
    pub regions: Vec<AnalyzerRegion>,
    pub error: Option<String>,
}

/// Ordered control frames; RGB bytes immediately follow each analyze command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AnalyzerWorkerCommand {
    Analyze { request: AnalyzerWorkerRequest },
    Cancel { request_id: u64 },
}
