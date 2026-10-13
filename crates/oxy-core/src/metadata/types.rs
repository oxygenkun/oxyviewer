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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MetadataProvider {
    Sidecar,
    Native,
    Exiftool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataCapability {
    pub provider: MetadataProvider,
    pub readable: bool,
    pub writable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
#[serde(rename_all = "camelCase")]
pub struct AssetDetailsResult {
    pub details: AssetDetails,
    pub metadata_projection: MetadataProjection,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MetadataPatch {
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub rating: Option<Option<u8>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub color_label: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub pick_label: Option<Option<PickLabel>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub title: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub creator: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub copyright: Option<Option<String>>,
    pub keywords: Option<Vec<String>>,
    pub hierarchical_keywords: Option<Vec<String>>,
}

fn deserialize_nullable_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
