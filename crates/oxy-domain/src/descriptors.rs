use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayDescriptor {
    pub id: String,
    pub coordinate_space: OverlayCoordinateSpace,
    pub items: Vec<OverlayRegion>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlayCoordinateSpace {
    DisplayNormalized,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayRegion {
    pub id: String,
    pub rect: crate::NormalizedRect,
    pub label: Option<String>,
    pub state: Option<String>,
    pub action_ref: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionViewDescriptor {
    pub id: String,
    pub source: String,
    pub selection: CollectionSelection,
    pub fields: Vec<CollectionField>,
    pub actions: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CollectionSelection {
    Single,
    Multi,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionField {
    pub key: String,
    pub label: String,
    pub kind: CollectionFieldKind,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CollectionFieldKind {
    Text,
    Percent,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDescriptor {
    pub id: String,
    pub fields: Vec<SettingField>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingField {
    pub key: String,
    pub label: String,
    pub recompute: crate::FaceRecomputeScope,
    #[serde(flatten)]
    pub input: SettingInput,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SettingInput {
    Number { min: f32, max: f32, step: f32 },
    Boolean,
    Enum { values: Vec<String> },
}
