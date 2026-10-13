//! Public types owned by the app feature.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
mod about;
pub use about::*;

/// One automated performance scenario injected into the packaged app through
/// the `OXY_PERF_SCENARIO` environment variable. Consumed by the frontend
/// performance harness; see `docs/PERF_E2E.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfScenario {
    #[serde(default)]
    pub expected_assets: Option<usize>,
    pub name: String,
    pub folder: PathBuf,
    #[serde(default)]
    pub select_name: Option<String>,
    #[serde(default)]
    pub enter_loupe: Option<bool>,
    #[serde(default)]
    pub scroll_to_end: bool,
    #[serde(default)]
    pub resource_stress: Option<String>,
    #[serde(default)]
    pub await_marks: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    pub report_path: PathBuf,
}
