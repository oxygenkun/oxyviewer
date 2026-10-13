//! Public types owned by the resources feature.
use oxy_media::types::HeifDecodeSession;
use oxy_media::types::HeifDecodeStatus;
use oxy_media::types::HeifDiagnostics;
use oxy_media::types::HeifTilePayload;
use oxy_media::types::PickLabel;
use oxy_media::types::PreviewPriority;
use oxy_media::types::PreviewResult;
use oxy_media::types::RenderLevel;
use oxy_media::types::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Rust-owned, versioned metadata read model shared by every UI surface.
/// A missing rating or color label is an authoritative empty value when
/// `status` is `Ready`, rather than a field that a caller may freely replace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataProjection {
    pub path: PathBuf,
    pub source_revision: String,
    /// Monotonic sequence assigned whenever Rust accepts a new state snapshot.
    pub state_revision: u64,
    pub valid_at: u64,
    pub status: ResourceLoadStatus,
    pub rating: Option<u8>,
    pub color_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick_label: Option<PickLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ResourceLoadStatus {
    Loading,
    Ready,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum MetadataRequestPriority {
    Background,
    Filter,
    Visible,
    Selected,
}

/// Read-only diagnostics for one queued or running unit of work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DebugQueueItem {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_path: Option<PathBuf>,
    pub stage: String,
    pub priority: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<i64>,
    pub consumers: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_count: Option<usize>,
}

/// A point-in-time view of a scheduler. This contract is intentionally
/// read-only so diagnostics cannot alter production scheduling.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DebugQueueState {
    pub name: String,
    pub concurrency: usize,
    pub pending: Vec<DebugQueueItem>,
    pub active: Vec<DebugQueueItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DebugQueueSnapshot {
    pub captured_at_unix_ms: u64,
    pub worker_wait_micros: u64,
    pub collection_micros: u64,
    pub stale_queues: Vec<String>,
    pub queues: Vec<DebugQueueState>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SchedulePlacement {
    Front,
    Back,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OmittedScheduleAction {
    Release,
    Demote,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewScheduleIntent {
    pub path: PathBuf,
    pub level: RenderLevel,
    pub priority: PreviewPriority,
    pub rank: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOmittedPolicy {
    pub action: OmittedScheduleAction,
    pub priority: Option<PreviewPriority>,
    pub placement: Option<SchedulePlacement>,
}

/// Rust-owned state for one semantic render level of an asset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageProjection {
    pub path: PathBuf,
    pub source_revision: String,
    /// Monotonic sequence assigned whenever Rust accepts a new state snapshot.
    pub state_revision: u64,
    pub valid_at: u64,
    pub status: ResourceLoadStatus,
    pub level: RenderLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<PreviewResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// User-visible policy and current state for the rebuildable preview cache.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CacheSettings {
    /// The app-owned directory that contains preview artifacts.
    pub location: PathBuf,
    pub default_location: PathBuf,
    pub custom_parent: Option<PathBuf>,
    pub is_custom_location: bool,
    pub max_size_bytes: u64,
    pub used_size_bytes: u64,
}

/// Mutable cache preferences. `custom_parent = None` restores the platform
/// default; custom parents always receive an app-owned child directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CacheSettingsUpdate {
    pub custom_parent: Option<PathBuf>,
    pub max_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "delivery", rename_all = "camelCase")]
pub enum HeifFullPresentation {
    Artifact { projection: Box<ImageProjection> },
    Tiles { session: HeifDecodeSession },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HeifTileReady {
    pub session_id: SessionId,
    pub generation: u64,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub payload: HeifTilePayload,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HeifStatusEvent {
    pub session_id: SessionId,
    pub generation: u64,
    pub status: HeifDecodeStatus,
    pub diagnostics: Option<HeifDiagnostics>,
    pub message: Option<String>,
}
