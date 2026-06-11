use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type AssetId = String;
pub type JobId = String;
pub type SessionId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderSession {
    pub id: SessionId,
    pub root_path: PathBuf,
    pub display_name: String,
    pub opened_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySummary {
    pub path: PathBuf,
    pub name: String,
    pub has_children: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Raw,
    Jpeg,
    Heif,
    Png,
    Tiff,
    Webp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditableMetadata {
    pub rating: Option<u8>,
    pub color_label: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetDetails {
    pub asset: AssetSummary,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub metadata: EditableMetadata,
    pub sidecar_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssetQuery {
    pub search: Option<String>,
    pub kind: Option<AssetKind>,
    pub sort: AssetSort,
    pub direction: SortDirection,
    pub page_size: Option<usize>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AssetSort {
    #[default]
    Name,
    Modified,
    Size,
    Kind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<usize>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MetadataPatch {
    pub rating: Option<Option<u8>>,
    pub color_label: Option<Option<String>>,
    pub title: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub creator: Option<Option<String>>,
    pub copyright: Option<Option<String>>,
    pub keywords: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FileOperation {
    Rename {
        source: PathBuf,
        new_name: String,
    },
    Copy {
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
    },
    Move {
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
    },
    Trash {
        paths: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOperationResult {
    pub affected_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum JobPriority {
    LibraryIndex,
    DirectoryBackground,
    SelectedMetadata,
    VisibleThumbnail,
    LoupePreview,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    pub job_id: JobId,
    pub completed: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
}
