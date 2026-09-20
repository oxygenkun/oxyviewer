//! The single checked-in source of truth for user-managed face models.

use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::FaceModelPaths;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedModelManifest {
    pub detector_input_size: u32,
    pub models: Vec<ManagedModelSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedModelSpec {
    pub id: String,
    pub display_name: String,
    pub file: String,
    pub url: Option<String>,
    pub archive_url: Option<String>,
    pub archive_sha256: Option<String>,
    pub archive_size_bytes: Option<u64>,
    pub archive_entry: Option<String>,
    pub sha256: String,
    pub size_bytes: u64,
    pub license_summary: String,
}

pub fn managed_model_manifest() -> &'static ManagedModelManifest {
    static MANIFEST: OnceLock<ManagedModelManifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str(include_str!("../../../3rdpart/face-models/managed.json"))
            .expect("checked-in managed face model manifest must be valid")
    })
}

pub fn managed_model_paths(directory: &Path) -> Option<FaceModelPaths> {
    let manifest = managed_model_manifest();
    let detector = manifest
        .models
        .iter()
        .find(|model| model.id == "scrfd-10g-kps")?;
    let embedder = manifest
        .models
        .iter()
        .find(|model| model.id == "adaface-ir101")?;
    let detector = directory.join(&detector.file);
    let embedder = directory.join(&embedder.file);
    (detector.is_file() && embedder.is_file())
        .then(|| FaceModelPaths::new(detector, embedder, manifest.detector_input_size))
}
