use crate::{jobs::schedule_library_index, state::AppState};
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub(crate) fn add_library_root(
    path: PathBuf,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<PathBuf>, String> {
    let root = path.canonicalize().map_err(|error| error.to_string())?;
    state
        .library
        .add_root(&root)
        .map_err(|error| error.to_string())?;
    schedule_library_index(app, state.library.clone(), root);
    state.library.roots().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn remove_library_root(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<Vec<PathBuf>, String> {
    state
        .library
        .remove_root(&path)
        .map_err(|error| error.to_string())?;
    state.library.roots().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn list_library_roots(state: State<'_, AppState>) -> Result<Vec<PathBuf>, String> {
    state.library.roots().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn reorder_library_roots(
    paths: Vec<PathBuf>,
    state: State<'_, AppState>,
) -> Result<Vec<PathBuf>, String> {
    state
        .library
        .reorder_roots(&paths)
        .map_err(|error| error.to_string())?;
    state.library.roots().map_err(|error| error.to_string())
}
