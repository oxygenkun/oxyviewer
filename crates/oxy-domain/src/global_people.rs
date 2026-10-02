//! App-wide identities and folder projections over individual person tuples.
use crate::{AssetSummary, PersonReviewDecision};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GlobalPerson {
    pub id: String,
    pub display_name: String,
    pub revision: i64,
    pub reference_instance_ids: Vec<String>,
    pub tag_id: Option<i64>,
}

/// Paged human-confirmed history, retained even when a source is offline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalPersonGallery {
    pub tuples: Vec<PersonTuple>,
    pub total: u64,
    pub folder_count: u64,
}

/// One folder in a person's confirmed history, independent of source availability.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalPersonFolder {
    pub folder_path: PathBuf,
    pub cover_asset_path: PathBuf,
    pub photo_count: u64,
    pub instance_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalPersonFolders {
    pub folders: Vec<GlobalPersonFolder>,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetGlobalPersonTag {
    pub person_id: String,
    pub tag_id: Option<i64>,
    pub expected_revision: i64,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonTuple {
    pub id: String,
    pub asset_path: PathBuf,
    pub source_revision: String,
    pub source_identity_revision: String,
    pub face_box: Option<[f64; 4]>,
    pub body_box: Option<[f64; 4]>,
    pub revision: i64,
    pub needs_review: bool,
    pub person_id: Option<String>,
    pub decision: Option<PersonReviewDecision>,
    pub score: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonTupleGroup {
    pub id: String,
    pub person_id: Option<String>,
    pub members: Vec<PersonTuple>,
    pub cover: Option<AssetSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FolderPeopleWorkspace {
    pub folder_path: PathBuf,
    pub revision: String,
    pub groups: Vec<PersonTupleGroup>,
    pub unknown_count: usize,
    pub known_count: usize,
    pub no_face_count: usize,
    pub unavailable_count: usize,
    pub has_analysis: bool,
    pub notice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersonTupleFilter {
    pub group_id: String,
    pub decision: Option<PersonReviewDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveGlobalPerson {
    pub id: Option<String>,
    pub display_name: String,
    pub expected_revision: i64,
    pub request_id: String,
}

/// The exact tuples the user saw, including their geometry/source revisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPersonTuples {
    pub folder_path: PathBuf,
    pub workspace_revision: String,
    pub tuples: Vec<PersonTuple>,
    pub person_id: String,
    pub decision: PersonReviewDecision,
    pub replace_confirmed: bool,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetGlobalPersonReference {
    pub person_id: String,
    pub instance_id: String,
    pub enabled: bool,
    pub expected_revision: i64,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPeopleGrouping {
    pub folder_path: PathBuf,
    /// None groups the folder; Some only retrieves the selected identities.
    pub person_ids: Option<Vec<String>>,
    /// An explicit candidate threshold, never an identity acceptance rule.
    pub similarity: f32,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePersonTupleGeometry {
    pub folder_path: PathBuf,
    pub asset_path: PathBuf,
    pub instance_id: Option<String>,
    pub expected_revision: i64,
    pub source_revision: String,
    pub face_box: Option<[f64; 4]>,
    pub body_box: Option<[f64; 4]>,
    pub request_id: String,
}
