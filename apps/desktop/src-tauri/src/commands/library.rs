use crate::state::AppState;
use oxy_core::{
    contracts::{RootRelocationPlan, RootRelocationResult},
    library::LibraryService,
};
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub(crate) async fn add_library_root(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<Vec<PathBuf>, String> {
    let service = state.library_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.add_root(&path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn remove_library_root(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<Vec<PathBuf>, String> {
    let service = state.library_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.remove_root(&path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn list_library_roots(state: State<'_, AppState>) -> Result<Vec<PathBuf>, String> {
    let service = state.library_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.roots())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn reorder_library_roots(
    paths: Vec<PathBuf>,
    state: State<'_, AppState>,
) -> Result<Vec<PathBuf>, String> {
    let service = state.library_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.reorder_roots(&paths))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn plan_root_relocation(
    old_root: PathBuf,
    new_root: PathBuf,
    state: State<'_, AppState>,
) -> Result<RootRelocationPlan, String> {
    let service = state.library_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.plan_relocation(&old_root, &new_root))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn relocate_library_root(
    plan: RootRelocationPlan,
    state: State<'_, AppState>,
) -> Result<RootRelocationResult, String> {
    let service = state.library_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.relocate(&plan))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn check_library_root(path: PathBuf) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || LibraryService::check_root(&path))
        .await
        .map_err(|error| error.to_string())?
}
