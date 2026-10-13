use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RelocationMatch {
    Verified,
    Unverified,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelocationEntry {
    pub old_path: PathBuf,
    pub new_path: PathBuf,
    pub status: RelocationMatch,
    pub old_identity_revision: Option<String>,
    pub new_identity_revision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RootRelocationPlan {
    pub old_root: PathBuf,
    pub new_root: PathBuf,
    pub entries: Vec<RelocationEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootRelocationResult {
    pub root_path: PathBuf,
    pub linked_files: usize,
    pub needs_review: usize,
    pub missing_files: usize,
    pub reused_artifacts: usize,
    pub cache_failures: usize,
}
