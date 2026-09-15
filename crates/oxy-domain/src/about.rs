use serde::{Deserialize, Serialize};

/// Build metadata shown in the About settings panel.
///
/// The version comes from the running package, so a rebuilt bundle cannot drift
/// from the number the About panel reports.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub repository_url: String,
    pub author: Option<String>,
    pub license: Option<String>,
}

/// Result of one user-requested release check.
///
/// The check is advisory: OxyViewer reports the newest published release and
/// links to it, and never downloads or installs anything from this path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
}

/// Fixed external destinations the About panel is allowed to open.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AboutLink {
    Repository,
    Releases,
    License,
}
