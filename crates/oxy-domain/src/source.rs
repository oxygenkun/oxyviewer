use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Canonical identity of source bytes; policy and derived-data versions are separate.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRevision {
    pub canonical_path: PathBuf,
    pub file_identity: String,
    pub size_bytes: u64,
    pub modified: String,
    pub revision_id: String,
}
