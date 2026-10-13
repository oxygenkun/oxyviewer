//! Public types owned by the files feature.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
mod external_apps;
pub use external_apps::*;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum FileDeletionMode {
    #[default]
    Trash,
    Permanent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
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
    DeletePermanently {
        paths: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FileOperationResult {
    pub affected_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "contracts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FileOperationRecovery {
    pub id: i64,
    pub operation: FileOperation,
    pub phase: String,
    pub detail: Option<String>,
}
