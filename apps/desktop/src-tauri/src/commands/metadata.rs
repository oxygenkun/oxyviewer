use crate::{providers::exiftool::ExiftoolStatus, state::AppState};
use oxy_domain::{
    AssetDetailsResult, AssetKind, JobId, JobPriority, MetadataPatch, MetadataProjection,
    MetadataRequestPriority,
};
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub(crate) async fn get_asset_details(
    path: PathBuf,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<AssetDetailsResult, String> {
    let queue = state.metadata_queue.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let receiver = queue.request_details(&app, path)?;
        receiver
            .recv()
            .map_err(|error| format!("metadata queue stopped: {error}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn request_metadata(
    paths: Vec<PathBuf>,
    priority: MetadataRequestPriority,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<MetadataProjection>, String> {
    let queue = state.metadata_queue.clone();
    tauri::async_runtime::spawn_blocking(move || queue.request_summaries(&app, paths, priority))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn patch_metadata(
    paths: Vec<PathBuf>,
    patch: MetadataPatch,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<JobId, String> {
    let ticket = state.jobs.register(JobPriority::SelectedMetadata);
    let job_id = ticket.id.clone();
    let files = state.files.clone();
    let metadata = state.metadata.clone();
    let requested_paths = paths.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        for path in paths {
            let asset = files.get_asset(&path).map_err(|error| error.to_string())?;
            metadata
                .patch_metadata(&path, asset.kind, &patch)
                .map_err(|error| error.to_string())?;
            if let Some(parent) = path.parent() {
                files.invalidate_directory(parent);
            }
        }
        Ok::<_, String>(())
    })
    .await
    .map_err(|error| error.to_string());
    state.jobs.finish(&job_id);
    result??;
    let queue = state.metadata_queue.clone();
    tauri::async_runtime::spawn_blocking(move || {
        queue.request_summaries(&app, requested_paths, MetadataRequestPriority::Selected)
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(job_id)
}

#[tauri::command]
pub(crate) async fn sync_metadata_to_embedded(
    paths: Vec<PathBuf>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<JobId, String> {
    let ticket = state.jobs.register(JobPriority::SelectedMetadata);
    let job_id = ticket.id.clone();
    let files = state.files.clone();
    let metadata = state.metadata.clone();
    let requested_paths = paths.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        for path in paths {
            let asset = files.get_asset(&path).map_err(|error| error.to_string())?;
            if asset.kind == AssetKind::Raw {
                continue;
            }
            metadata
                .sync_metadata_to_embedded(&path)
                .map_err(|error| error.to_string())?;
            if let Some(parent) = path.parent() {
                files.invalidate_directory(parent);
            }
        }
        Ok::<_, String>(())
    })
    .await
    .map_err(|error| error.to_string());
    state.jobs.finish(&job_id);
    result??;
    let queue = state.metadata_queue.clone();
    tauri::async_runtime::spawn_blocking(move || {
        queue.request_summaries(&app, requested_paths, MetadataRequestPriority::Selected)
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(job_id)
}

#[tauri::command]
pub(crate) async fn get_exiftool_status(
    state: State<'_, AppState>,
) -> Result<ExiftoolStatus, String> {
    let provider = state.metadata_provider.clone();
    tauri::async_runtime::spawn_blocking(move || provider.status())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn configure_exiftool(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<ExiftoolStatus, String> {
    let provider = state.metadata_provider.clone();
    tauri::async_runtime::spawn_blocking(move || provider.set_user_executable(path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn install_exiftool(state: State<'_, AppState>) -> Result<ExiftoolStatus, String> {
    let provider = state.metadata_provider.clone();
    tauri::async_runtime::spawn_blocking(move || provider.install_managed())
        .await
        .map_err(|error| error.to_string())?
}
