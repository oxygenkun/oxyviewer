//! Public types owned by the library feature.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
mod relocation;
pub use relocation::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryIndexUpdate {
    pub root_path: PathBuf,
    pub asset_count: usize,
    pub directory_count: usize,
}
