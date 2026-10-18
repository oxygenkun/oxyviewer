//! Public types owned by the metadata feature.
use crate::browse::types::AssetSummary;
use crate::resources::types::MetadataProjection;
use oxy_media::types::CaptureMetadata;
use oxy_media::types::EditableMetadata;
use oxy_media::types::FocusInfo;
use oxy_media::types::PickLabel;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
mod burst;
pub use burst::*;

/// Host-side execution policy; not an IPC payload or persisted user setting.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedSyncOptions {
    /// Each lane owns one ExifTool session. Accepted range is 1..=4.
    pub concurrency: usize,
}

impl Default for EmbeddedSyncOptions {
    fn default() -> Self {
        Self { concurrency: 2 }
    }
}

/// One file whose embedded sync did not complete successfully. An uncertain
/// result may already have changed the file and must never be retried implicitly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedSyncFailure {
    pub path: PathBuf,
    pub error: String,
    pub uncertain: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedSyncResult {
    pub job_id: crate::jobs::types::JobId,
    pub total: usize,
    pub completed: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub cancelled: bool,
    pub failures: Vec<EmbeddedSyncFailure>,
}

/// Small per-file updates; the command's final result remains authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedSyncProgress {
    pub request_id: String,
    pub job_id: crate::jobs::types::JobId,
    pub total: usize,
    pub completed: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum MetadataProvider {
    Sidecar,
    Native,
    Exiftool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct MetadataCapability {
    pub provider: MetadataProvider,
    pub readable: bool,
    pub writable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AssetDetails {
    pub asset: AssetSummary,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub metadata: EditableMetadata,
    #[serde(default)]
    pub embedded_keywords: Vec<String>,
    #[serde(default)]
    pub embedded_hierarchical_keywords: Vec<String>,
    pub metadata_capability: MetadataCapability,
    pub sidecar_path: Option<PathBuf>,
    #[serde(default)]
    pub capture_metadata: CaptureMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_info: Option<FocusInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AssetDetailsResult {
    pub details: AssetDetails,
    pub metadata_projection: MetadataProjection,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct MetadataPatch {
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    #[cfg_attr(feature = "contracts", ts(optional))]
    pub rating: Option<Option<u8>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    #[cfg_attr(feature = "contracts", ts(optional))]
    pub color_label: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    #[cfg_attr(feature = "contracts", ts(optional))]
    pub pick_label: Option<Option<PickLabel>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    #[cfg_attr(feature = "contracts", ts(optional))]
    pub title: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    #[cfg_attr(feature = "contracts", ts(optional))]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    #[cfg_attr(feature = "contracts", ts(optional))]
    pub creator: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    #[cfg_attr(feature = "contracts", ts(optional))]
    pub copyright: Option<Option<String>>,
    #[cfg_attr(feature = "contracts", ts(optional))]
    pub keywords: Option<Vec<String>>,
    #[cfg_attr(feature = "contracts", ts(optional))]
    pub hierarchical_keywords: Option<Vec<String>>,
}

fn deserialize_nullable_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
