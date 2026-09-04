use crate::{
    commands::schedule_tag_xmp_sync, jobs::directory_tree::DIRECTORY_TREE_UPDATED_EVENT,
    state::AppState,
};
use oxy_domain::{
    AssetQuery, AssetSummary, DirectorySearchMatch, DirectoryTreeSnapshot, FileOperation,
    FileOperationResult, FolderSession, Page,
};
use std::path::PathBuf;
use tauri::{Emitter, State};

#[tauri::command]
pub(crate) async fn open_folder(
    path: PathBuf,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<FolderSession, String> {
    let files = state.files.clone();
    let library = state.library.clone();
    let (session, should_index) = tauri::async_runtime::spawn_blocking(move || {
        let session = files.open_folder(path).map_err(|error| error.to_string())?;
        let should_index = library.contains_root(&session.root_path).unwrap_or(false);
        Ok::<_, String>((session, should_index))
    })
    .await
    .map_err(|error| error.to_string())??;
    if should_index {
        state
            .library_index_queue
            .schedule(app, session.root_path.clone());
    }
    schedule_tag_xmp_sync(
        state.library.clone(),
        state.files.clone(),
        state.metadata.clone(),
    );
    Ok(session)
}

#[tauri::command]
pub(crate) async fn list_assets(
    session_id: String,
    directory: Option<PathBuf>,
    query: AssetQuery,
    cursor: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Page<AssetSummary>, String> {
    let files = state.files.clone();
    let metadata = state.metadata.clone();
    let library = state.library.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if query.minimum_rating.is_some() || !query.color_labels.is_empty() {
            let mut assets = files
                .list_asset_candidates(&session_id, directory.as_deref())
                .map_err(|error| error.to_string())?;
            metadata
                .enrich_summaries(&mut assets)
                .map_err(|error| error.to_string())?;
            Ok(oxy_fs::page_assets(&assets, &query, cursor.unwrap_or(0)))
        } else {
            let root = files
                .session_root(&session_id)
                .map_err(|error| error.to_string())?;
            let resolved_directory = files
                .session_directory(&session_id, directory.as_deref())
                .map_err(|error| error.to_string())?;
            if let Some(page) = library
                .list_assets(&root, &resolved_directory, &query, cursor.unwrap_or(0))
                .map_err(|error| error.to_string())?
            {
                return Ok(page);
            }
            files
                .list_assets(&session_id, directory.as_deref(), &query, cursor)
                .map_err(|error| error.to_string())
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn get_directory_tree(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<DirectoryTreeSnapshot, String> {
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        files
            .directory_tree(&session_id)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) fn set_directory_expanded(
    session_id: String,
    directory: PathBuf,
    expanded: bool,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DirectoryTreeSnapshot, String> {
    let (snapshot, should_load) = state
        .files
        .set_directory_expanded(&session_id, &directory, expanded)
        .map_err(|error| error.to_string())?;
    let _ = app.emit(DIRECTORY_TREE_UPDATED_EVENT, snapshot.clone());
    if should_load {
        state.directory_tree_queue.enqueue(session_id, directory);
    }
    Ok(snapshot)
}

#[tauri::command]
pub(crate) async fn set_active_directory(
    session_id: String,
    directory: PathBuf,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let files = state.files.clone();
    let (session_id, directory) = tauri::async_runtime::spawn_blocking(move || {
        files
            .session_directory(&session_id, Some(&directory))
            .map(|directory| (session_id, directory))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;
    state
        .metadata_queue
        .clear_pending_outside_directory(&directory);
    state
        .preview_queue
        .clear_pending_outside_directory(&directory);
    state.directory_tree_queue.set_active(session_id, directory);
    Ok(())
}

#[tauri::command]
pub(crate) async fn search_directories(
    session_id: String,
    search: String,
    state: State<'_, AppState>,
) -> Result<Option<Vec<DirectorySearchMatch>>, String> {
    let files = state.files.clone();
    let library = state.library.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = files
            .session_root(&session_id)
            .map_err(|error| error.to_string())?;
        library
            .search_directories(&root, &search)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn refresh_directory(
    session_id: String,
    directory: Option<PathBuf>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DirectoryTreeSnapshot, String> {
    let files = state.files.clone();
    let (tree, root, resolved_directory) = tauri::async_runtime::spawn_blocking(move || {
        let root = files
            .session_root(&session_id)
            .map_err(|error| error.to_string())?;
        let resolved_directory = files
            .session_directory(&session_id, directory.as_deref())
            .map_err(|error| error.to_string())?;
        let tree = files
            .refresh_directory(&session_id, directory.as_deref())
            .map_err(|error| error.to_string())?;
        Ok::<_, String>((tree, root, resolved_directory))
    })
    .await
    .map_err(|error| error.to_string())??;
    state
        .library
        .invalidate_resource_projections(&resolved_directory)
        .map_err(|error| error.to_string())?;
    state
        .metadata_queue
        .invalidate_directory(&resolved_directory);
    state
        .preview_queue
        .invalidate_directory(&resolved_directory);
    state
        .library
        .invalidate_index(&root)
        .map_err(|error| error.to_string())?;
    state.library_index_queue.schedule(app, root);
    Ok(tree)
}

#[tauri::command]
pub(crate) async fn execute_file_operation(
    operation: FileOperation,
    state: State<'_, AppState>,
) -> Result<FileOperationResult, String> {
    let library = state.library.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result =
            oxy_fs::execute_file_operation(&operation).map_err(|error| error.to_string())?;
        match &operation {
            FileOperation::Rename { source, new_name } => {
                let destination = source
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new(""))
                    .join(new_name);
                library
                    .move_asset_tag_state(source, &destination)
                    .map_err(|error| error.to_string())?;
            }
            FileOperation::Copy {
                sources,
                destination_dir,
            } => {
                for source in sources {
                    if let Some(name) = source.file_name() {
                        library
                            .copy_asset_tag_state(source, &destination_dir.join(name))
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
                        library
                            .move_asset_tag_state(source, &destination_dir.join(name))
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
            FileOperation::Trash { paths } => {
                for path in paths {
                    library
                        .remove_asset_tag_state(path)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        Ok(result)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn open_in_file_manager(path: PathBuf) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        oxy_fs::open_in_file_manager(&path).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
