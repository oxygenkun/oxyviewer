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
    let library = state.library.clone();
    let tags = state.tags.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = oxy_core::files::execute_file_operation(&operation)
            .map_err(|error| error.to_string())?;
        match &operation {
            FileOperation::Rename { source, new_name } => {
                let destination = source
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new(""))
                    .join(new_name);
                tags.move_asset_state(source, &destination)
                    .map_err(|error| error.to_string())?;
            }
            FileOperation::Copy {
                sources,
                destination_dir,
            } => {
                for source in sources {
                    if let Some(name) = source.file_name() {
                        tags.copy_asset_state(source, &destination_dir.join(name))
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
            FileOperation::Move {
                sources,
                destination_dir,
            } => {
                for source in sources {
                    if let Some(name) = source.file_name() {
                        tags.move_asset_state(source, &destination_dir.join(name))
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
            FileOperation::Trash { paths } | FileOperation::DeletePermanently { paths } => {
                for path in paths {
                    tags.remove_asset_state(path)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        for path in &result.affected_paths {
            let directory = path.parent().unwrap_or(path);
            library
                .invalidate_directory_snapshots(directory)
                .map_err(|error| error.to_string())?;
        }
        let sources = match &operation {
            FileOperation::Rename { source, .. } => std::slice::from_ref(source),
            FileOperation::Copy { sources, .. } | FileOperation::Move { sources, .. } => {
                sources.as_slice()
            }
            FileOperation::Trash { paths } | FileOperation::DeletePermanently { paths } => {
                paths.as_slice()
            }
        };
        for path in sources {
            library
                .invalidate_directory_snapshots(path.parent().unwrap_or(path))
                .map_err(|error| error.to_string())?;
        }
        Ok(result)
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
