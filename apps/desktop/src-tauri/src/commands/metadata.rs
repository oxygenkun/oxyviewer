use crate::state::AppState;
use oxy_core::contracts::{
    AssetDetailsResult, EmbeddedSyncResult, JobId, MetadataPatch, MetadataProjection,
    MetadataRequestPriority,
};
use oxy_core::metadata::provider::ExiftoolStatus;
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub(crate) async fn get_asset_details(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<AssetDetailsResult, String> {
    let service = state.metadata_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.details(path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn request_metadata(
    paths: Vec<PathBuf>,
    priority: MetadataRequestPriority,
    state: State<'_, AppState>,
) -> Result<Vec<MetadataProjection>, String> {
    let service = state.metadata_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.request(paths, priority))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn patch_metadata(
    paths: Vec<PathBuf>,
    patch: MetadataPatch,
    state: State<'_, AppState>,
) -> Result<JobId, String> {
    let receiver = state.metadata_service.patch(paths, patch)?;
    tauri::async_runtime::spawn_blocking(move || {
        receiver.recv().map_err(|error| error.to_string())?
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn sync_metadata_to_embedded(
    paths: Vec<PathBuf>,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<EmbeddedSyncResult, String> {
    let receiver = state.metadata_service.sync_to_embedded(paths, request_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        receiver.recv().map_err(|error| error.to_string())?
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn get_exiftool_status(
    state: State<'_, AppState>,
) -> Result<ExiftoolStatus, String> {
    let receiver = state.metadata_service.provider_status()?;
    tauri::async_runtime::spawn_blocking(move || {
        receiver.recv().map_err(|error| error.to_string())?
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn configure_exiftool(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<ExiftoolStatus, String> {
    let receiver = state.metadata_service.configure_provider(path)?;
    tauri::async_runtime::spawn_blocking(move || {
        receiver.recv().map_err(|error| error.to_string())?
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn install_exiftool(state: State<'_, AppState>) -> Result<ExiftoolStatus, String> {
    let receiver = state.metadata_service.install_provider()?;
    tauri::async_runtime::spawn_blocking(move || {
        receiver.recv().map_err(|error| error.to_string())?
    })
    .await
    .map_err(|error| error.to_string())?
}
