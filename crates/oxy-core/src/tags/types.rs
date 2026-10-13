//! Public types owned by the tags feature.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type CustomTagId = i64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CustomTag {
    pub id: CustomTagId,
    pub parent_id: Option<CustomTagId>,
    pub name: String,
    pub path: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AssetTagAssignment {
    pub tag: CustomTag,
    pub assigned_count: usize,
    pub asset_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AssetTagAssignmentsByPath {
    pub path: PathBuf,
    pub assignments: Vec<AssetTagAssignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TagDeleteImpact {
    pub tag_count: usize,
    pub asset_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TagSyncStatus {
    pub pending_count: usize,
    pub failed_count: usize,
    pub last_error: Option<String>,
}
