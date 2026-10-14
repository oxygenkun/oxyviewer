use crate::state::AppState;
use oxy_core::contracts::*;
use std::{path::PathBuf, sync::Arc};
use tauri::State;

#[tauri::command]
pub(crate) async fn get_global_person_folders(
    person_id: String,
    offset: u32,
    limit: u32,
    state: State<'_, AppState>,
) -> Result<GlobalPersonFolders, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .global_person_folders(&person_id, offset, limit)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn get_global_person_gallery(
    person_id: String,
    offset: u32,
    limit: u32,
    state: State<'_, AppState>,
) -> Result<GlobalPersonGallery, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .global_person_gallery(&person_id, offset, limit)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn set_global_person_tag(
    input: SetGlobalPersonTag,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let service = Arc::clone(&state.person_service);
    tauri::async_runtime::spawn_blocking(move || service.set_global_person_tag(input))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn get_global_person_references(
    person_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<PersonTuple>, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .global_person_reference_tuples(&person_id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn save_person_tuple_geometry(
    input: SavePersonTupleGeometry,
    state: State<'_, AppState>,
) -> Result<PersonTuple, String> {
    let service = Arc::clone(&state.person_service);
    tauri::async_runtime::spawn_blocking(move || service.save_tuple_geometry(input))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn list_global_people(
    state: State<'_, AppState>,
) -> Result<Vec<GlobalPerson>, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || people.global_people().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn get_folder_people_workspace(
    folder_path: PathBuf,
    state: State<'_, AppState>,
) -> Result<FolderPeopleWorkspace, String> {
    let service = Arc::clone(&state.person_service);
    tauri::async_runtime::spawn_blocking(move || service.workspace(folder_path))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn save_global_person(
    input: SaveGlobalPerson,
    state: State<'_, AppState>,
) -> Result<GlobalPerson, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people.save_global_person(&input).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn review_person_tuples(
    input: ReviewPersonTuples,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .review_person_tuples(&input)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn set_global_person_reference(
    input: SetGlobalPersonReference,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .set_global_person_reference(&input)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn get_person_tuple_asset(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<AssetSummary, String> {
    let service = Arc::clone(&state.person_service);
    tauri::async_runtime::spawn_blocking(move || service.tuple_asset(path))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) fn start_people_grouping(
    session_id: String,
    input: RunPeopleGrouping,
    refresh_features: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .person_service
        .start_grouping(session_id, input, refresh_features)
}
