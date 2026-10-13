use crate::state::AppState;
use oxy_core::contracts::{
    AssetQuery, BrowsePage, DirectorySearchMatch, DirectoryTreeSnapshot, FileOperation,
    FileOperationResult, FolderSession,
};
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub(crate) async fn open_folder(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<FolderSession, String> {
    let browse = state.browse.clone();
    tauri::async_runtime::spawn_blocking(move || browse.open_folder(path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn list_assets(
    session_id: String,
    directory: Option<PathBuf>,
    query: AssetQuery,
    cursor: Option<usize>,
    snapshot_revision: Option<u64>,
    state: State<'_, AppState>,
) -> Result<BrowsePage, String> {
    let browse = state.browse.clone();
    tauri::async_runtime::spawn_blocking(move || {
        browse.list_assets(session_id, directory, query, cursor, snapshot_revision)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn get_directory_tree(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<DirectoryTreeSnapshot, String> {
    let browse = state.browse.clone();
    tauri::async_runtime::spawn_blocking(move || browse.get_directory_tree(session_id))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn set_directory_expanded(
    session_id: String,
    directory: PathBuf,
    expanded: bool,
    state: State<'_, AppState>,
) -> Result<DirectoryTreeSnapshot, String> {
    let browse = state.browse.clone();
    tauri::async_runtime::spawn_blocking(move || {
        browse.set_directory_expanded(session_id, directory, expanded)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn collapse_directory_tree(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<DirectoryTreeSnapshot, String> {
    let browse = state.browse.clone();
    tauri::async_runtime::spawn_blocking(move || browse.collapse_directory_tree(session_id))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn set_active_directory(
    session_id: String,
    directory: PathBuf,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let browse = state.browse.clone();
    tauri::async_runtime::spawn_blocking(move || browse.set_active_directory(session_id, directory))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn search_directories(
    session_id: String,
    search: String,
    state: State<'_, AppState>,
) -> Result<Option<Vec<DirectorySearchMatch>>, String> {
    let browse = state.browse.clone();
    tauri::async_runtime::spawn_blocking(move || browse.search_directories(session_id, search))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn refresh_directory(
    session_id: String,
    directory: Option<PathBuf>,
    state: State<'_, AppState>,
) -> Result<DirectoryTreeSnapshot, String> {
    let browse = state.browse.clone();
    tauri::async_runtime::spawn_blocking(move || browse.refresh_directory(session_id, directory))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn execute_file_operation(
    operation: FileOperation,
    state: State<'_, AppState>,
) -> Result<FileOperationResult, String> {
    let receiver = state.file_service.execute(operation)?;
    tauri::async_runtime::spawn_blocking(move || {
        receiver.recv().map_err(|error| error.to_string())?
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn list_file_operation_recovery(
    state: State<'_, AppState>,
) -> Result<Vec<oxy_core::contracts::FileOperationRecovery>, String> {
    let service = state.file_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.pending_recovery())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn recover_file_operation_state(
    id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let receiver = state.file_service.recover(id)?;
    tauri::async_runtime::spawn_blocking(move || {
        receiver.recv().map_err(|error| error.to_string())?
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn open_in_file_manager(path: PathBuf) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        oxy_core::files::open_in_file_manager(&path).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
