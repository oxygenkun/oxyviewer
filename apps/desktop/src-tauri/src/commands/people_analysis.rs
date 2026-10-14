use crate::state::AppState;
use oxy_core::contracts::*;
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub(crate) async fn get_person_detections(
    folder_path: PathBuf,
    asset_path: PathBuf,
    state: State<'_, AppState>,
) -> Result<PersonDetectionSnapshot, String> {
    let service = state.person_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.detections(folder_path, asset_path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn adopt_person_detection(
    folder_path: PathBuf,
    asset_path: PathBuf,
    source_revision: String,
    instance_id: String,
    state: State<'_, AppState>,
) -> Result<PersonInstance, String> {
    let service = state.person_service.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.adopt_detection(folder_path, asset_path, source_revision, instance_id)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn get_person_clusters(
    folder_path: PathBuf,
    state: State<'_, AppState>,
) -> Result<Option<PersonClusterSnapshot>, String> {
    let service = state.person_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.clusters(folder_path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn adopt_person_cluster(
    input: AdoptPersonCluster,
    state: State<'_, AppState>,
) -> Result<AdoptPersonClusterResult, String> {
    let service = state.person_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.adopt_cluster(input))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) fn start_person_clustering(
    session_id: String,
    folder_path: PathBuf,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .person_service
        .start_clustering(session_id, folder_path, request_id)
}

#[tauri::command]
pub(crate) fn start_person_model_download(
    model_id: String,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .person_service
        .start_model_download(model_id, request_id)
}

#[tauri::command]
pub(crate) fn import_person_model(
    model_id: String,
    path: PathBuf,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .person_service
        .import_model(model_id, path, request_id)
}

#[tauri::command]
pub(crate) fn start_folder_person_analysis(
    session_id: String,
    folder_path: PathBuf,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .person_service
        .start_analysis(session_id, folder_path, request_id)
}

#[tauri::command]
pub(crate) async fn get_person_models(
    state: State<'_, AppState>,
) -> Result<Vec<PersonModelStatus>, String> {
    let service = state.person_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.models())
        .await
        .map_err(|error| error.to_string())
}
#[tauri::command]
pub(crate) fn get_person_operation(
    state: State<'_, AppState>,
) -> Result<Option<PersonOperationStatus>, String> {
    state.person_service.operation()
}
#[tauri::command]
pub(crate) fn cancel_person_operation(
    operation_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.person_service.cancel_operation(&operation_id)
}
