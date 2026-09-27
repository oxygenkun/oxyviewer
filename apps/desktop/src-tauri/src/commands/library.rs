use crate::state::AppState;
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
    state.library_index_queue.schedule(app, root);
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

#[tauri::command]
pub(crate) async fn plan_root_relocation(
    old_root: PathBuf,
    new_root: PathBuf,
    state: State<'_, AppState>,
) -> Result<oxy_domain::RootRelocationPlan, String> {
    let library = std::sync::Arc::clone(&state.library);
    let cache_dir = state.cache.preview_dir();
    tauri::async_runtime::spawn_blocking(move || {
        let cache = oxy_media::DiskMediaCache::new(cache_dir, 256).map_err(|e| e.to_string())?;
        let sources = cache
            .relocation_sources(&old_root)
            .map_err(|e| e.to_string())?;
        let proofs = sources
            .into_iter()
            .map(|s| (s.canonical_path, s.revision_id))
            .collect::<Vec<_>>();
        library
            .plan_root_relocation(&old_root, &new_root, &proofs)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn relocate_library_root(
    plan: oxy_domain::RootRelocationPlan,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<oxy_domain::RootRelocationResult, String> {
    let library = std::sync::Arc::clone(&state.library);
    let cache_dir = state.cache.preview_dir();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let cache = oxy_media::DiskMediaCache::new(cache_dir, 256).map_err(|e| e.to_string())?;
        let sources = cache
            .relocation_sources(&plan.old_root)
            .map_err(|e| e.to_string())?;
        let proofs = sources
            .iter()
            .map(|s| (s.canonical_path.clone(), s.revision_id.clone()))
            .collect::<Vec<_>>();
        library
            .relocate_root(&plan, &proofs)
            .map_err(|e| e.to_string())?;
        let report = cache.reuse_root_relocation(&plan, &sources);
        Ok::<_, String>(report)
    })
    .await
    .map_err(|e| e.to_string())??;
    state
        .library_index_queue
        .schedule(app, result.root_path.clone());
    Ok(result)
}

#[tauri::command]
pub(crate) async fn check_library_root(path: PathBuf) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        oxy_fs::relocation_directory(&path)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
