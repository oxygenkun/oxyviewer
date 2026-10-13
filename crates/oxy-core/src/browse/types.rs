//! Public types owned by the browse feature.
use crate::files::types::FileDeletionMode;
use crate::people::types::PersonClusterFilter;
use crate::people::types::PersonFilter;
use crate::people::types::PersonTupleFilter;
use crate::tags::types::CustomTagId;
use oxy_media::types::AssetKind;
use oxy_media::types::PickLabel;
use oxy_media::types::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type AssetId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FolderSession {
    pub id: SessionId,
    pub root_path: PathBuf,
    pub display_name: String,
    pub opened_at_ms: u64,
    #[serde(default)]
    pub deletion_mode: FileDeletionMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DirectorySummary {
    pub path: PathBuf,
    pub name: String,
    pub has_children: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DirectoryTreeNode {
    pub entry: DirectorySummary,
    pub expanded: bool,
    /// `None` means this level has not been loaded yet. An empty vector is an
    /// authoritative leaf result.
    pub children: Option<Vec<DirectoryTreeNode>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DirectoryTreeSnapshot {
    pub session_id: SessionId,
    pub revision: u64,
    pub root: DirectoryTreeNode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DirectorySearchMatch {
    pub directory: DirectorySummary,
    pub ancestors: Vec<DirectorySummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AssetSummary {
    pub id: AssetId,
    pub path: PathBuf,
    pub name: String,
    pub extension: String,
    pub kind: AssetKind,
    pub size_bytes: u64,
    pub modified_at_ms: u64,
    pub has_sidecar: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick_label: Option<PickLabel>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum TagMatchMode {
    #[default]
    All,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AssetQuery {
    pub person_filter: Option<PersonFilter>,
    pub person_cluster_filter: Option<PersonClusterFilter>,
    pub person_tuple_filter: Option<PersonTupleFilter>,
    #[serde(default)]
    pub tag_ids: Vec<CustomTagId>,
    #[serde(default)]
    pub tag_match: TagMatchMode,
    pub search: Option<String>,
    pub kind: Option<AssetKind>,
    pub minimum_rating: Option<u8>,
    #[serde(default)]
    pub color_labels: Vec<String>,
    #[serde(default)]
    pub pick_labels: Vec<String>,
    pub sort: AssetSort,
    pub direction: SortDirection,
    pub page_size: Option<usize>,
}

impl AssetQuery {
    /// True when a filter cannot be answered from a cheap directory scan and
    /// needs sidecar/XMP enrichment first. Keep this the single source of truth:
    /// a caller that skips enrichment silently drops every matching asset.
    pub fn needs_metadata_enrichment(&self) -> bool {
        self.minimum_rating.is_some()
            || !self.color_labels.is_empty()
            || !self.pick_labels.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum AssetSort {
    #[default]
    Name,
    Modified,
    Size,
    Kind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<usize>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DirectoryBrowseProgress {
    pub session_id: String,
    pub directory: PathBuf,
    pub stage: String,
    pub source: String,
    pub discovered_count: usize,
    pub elapsed_ms: u64,
    pub cache_ms: u64,
    pub resolve_ms: u64,
    pub enumeration_ms: u64,
    pub attributes_ms: u64,
    pub snapshot_serialize_ms: u64,
    pub snapshot_persist_ms: u64,
    pub sort_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct BrowsePage {
    #[serde(flatten)]
    pub page: Page<AssetSummary>,
    pub progress: DirectoryBrowseProgress,
    pub snapshot_revision: Option<u64>,
}
