use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A model suggestion, never a confirmed identity or review decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonClusterMember {
    pub asset_path: PathBuf,
    pub instance_id: String,
    pub source_revision: String,
    pub summary_revision: String,
    pub face_box: [f64; 4],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonCluster {
    pub id: String,
    pub members: Vec<PersonClusterMember>,
    pub cover: Option<crate::AssetSummary>,
    #[serde(default)]
    pub people: Vec<crate::FolderPerson>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonClusterSnapshot {
    pub snapshot_id: String,
    pub folder_path: PathBuf,
    pub run_id: String,
    #[serde(default)]
    pub pipeline_fingerprint: String,
    pub algorithm: String,
    pub clusters: Vec<PersonCluster>,
    pub ungrouped: Vec<PersonClusterMember>,
    pub no_face_count: usize,
    pub unavailable_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersonClusterFilter {
    pub snapshot_id: String,
    /// "ungrouped" selects all singleton / ambiguous detections.
    pub cluster_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptPersonCluster {
    pub folder_path: PathBuf,
    pub snapshot_id: String,
    pub cluster_id: String,
    pub subject_id: Option<String>,
    pub display_name: Option<String>,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptPersonClusterResult {
    pub person: crate::FolderPerson,
    pub added: usize,
    pub preserved: usize,
    pub conflicted: usize,
}
