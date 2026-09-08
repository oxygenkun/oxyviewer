use crate::{
    commands::schedule_tag_xmp_sync, jobs::directory_tree::DIRECTORY_TREE_UPDATED_EVENT,
    state::AppState,
};
use oxy_domain::{
    AssetQuery, BrowsePage, DirectoryBrowseProgress, DirectorySearchMatch, DirectoryTreeSnapshot,
    FileOperation, FileOperationResult, FolderSession,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
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
        let _foreground = library.foreground.enter();
        let registered = library
            .roots()
            .map_err(|error| error.to_string())?
            .contains(&path);
        let session = if registered {
            files.restore_registered_folder(path)
        } else {
            files.open_folder(path).map_err(|error| error.to_string())?
        };
        let should_index = library
            .root_needs_index(&session.root_path)
            .unwrap_or(false);
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
    snapshot_revision: Option<u64>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<BrowsePage, String> {
    let files = state.files.clone();
    let metadata = state.metadata.clone();
    let library = state.library.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _foreground = library.foreground.enter();
        let started = Instant::now();
        let root = files
            .session_root(&session_id)
            .map_err(|error| error.to_string())?;
        let directory = directory.unwrap_or_else(|| root.clone());
        let mut progress = DirectoryBrowseProgress {
            session_id,
            directory: directory.clone(),
            stage: "cache".into(),
            ..Default::default()
        };
        let _ = app.emit("directory-browse-progress", &progress);
        // Preserve existing library-wide search semantics. Ordinary browsing
        // uses independent snapshots even during an incomplete root index.
        if query
            .search
            .as_ref()
            .is_some_and(|search| !search.is_empty())
        {
            if let Some(page) = library
                .list_assets(&root, &directory, &query, cursor.unwrap_or(0))
                .map_err(|error| error.to_string())?
            {
                progress.source = "index".into();
                progress.stage = "ready".into();
                progress.cache_ms = started.elapsed().as_millis() as u64;
                progress.elapsed_ms = progress.cache_ms;
                return Ok(BrowsePage {
                    page,
                    progress,
                    snapshot_revision: None,
                });
            }
        }
        let mut last_report = Instant::now();
        let mut scanning = false;
        let read = library
            .browse_directory_revision(&root, &directory, snapshot_revision, |scan| {
                if !scanning {
                    progress.cache_ms = started.elapsed().as_millis() as u64;
                    scanning = true;
                }
                progress.stage = if scan.resolving {
                    "resolving"
                } else if scan.reading_attributes {
                    "attributes"
                } else {
                    "enumerating"
                }
                .into();
                progress.discovered_count = scan.discovered_count;
                progress.enumeration_ms = scan.enumeration_ms;
                progress.resolve_ms = scan.resolve_ms;
                progress.attributes_ms = scan.attributes_ms;
                progress.elapsed_ms = started.elapsed().as_millis() as u64;
                if last_report.elapsed() >= Duration::from_millis(100) {
                    let _ = app.emit("directory-browse-progress", &progress);
                    last_report = Instant::now();
                }
            })
            .map_err(|error| error.to_string())?;
        if !scanning {
            progress.cache_ms = started.elapsed().as_millis() as u64;
        }
        progress.snapshot_serialize_ms = read.snapshot_serialize_ms;
        progress.snapshot_persist_ms = read.snapshot_persist_ms;
        progress.source = read.source.into();
        progress.stage = "sorting".into();
        let _ = app.emit("directory-browse-progress", &progress);
        let sort_started = Instant::now();
        let page = if query.minimum_rating.is_some() || !query.color_labels.is_empty() {
            let mut assets = read.assets.as_ref().clone();
            metadata
                .enrich_summaries(&mut assets)
                .map_err(|error| error.to_string())?;
            oxy_fs::page_assets(&assets, &query, cursor.unwrap_or(0))
        } else {
            oxy_fs::page_assets(&read.assets, &query, cursor.unwrap_or(0))
        };
        progress.sort_ms = sort_started.elapsed().as_millis() as u64;
        progress.discovered_count = read.assets.len();
        progress.stage = if read.error.is_some() {
            "stale"
        } else {
            "ready"
        }
        .into();
        progress.error = read.error;
        progress.elapsed_ms = started.elapsed().as_millis() as u64;
        let _ = app.emit("directory-browse-progress", &progress);
        if read.needs_validation {
            let library = library.clone();
            let mut background = progress.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let started = Instant::now();
                let mut last_report = Instant::now();
                let result = library.revalidate_directory(&root, &directory, read.epoch, |scan| {
                    background.stage = "validating".into();
                    background.discovered_count = scan.discovered_count;
                    background.enumeration_ms = scan.enumeration_ms;
                    background.resolve_ms = scan.resolve_ms;
                    background.attributes_ms = scan.attributes_ms;
                    if last_report.elapsed() >= Duration::from_millis(100) {
                        let _ = app.emit("directory-browse-progress", &background);
                        last_report = Instant::now();
                    }
                });
                background.elapsed_ms = started.elapsed().as_millis() as u64;
                match result {
                    Ok(true) => background.stage = "updated".into(),
                    Ok(false) => return,
                    Err(error) => {
                        background.stage = "stale".into();
                        background.error = Some(error.to_string());
                    }
                }
                let _ = app.emit("directory-browse-progress", background);
            });
        }
        Ok(BrowsePage {
            page,
            progress,
            snapshot_revision: Some(read.revision),
        })
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
pub(crate) fn collapse_directory_tree(
    session_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DirectoryTreeSnapshot, String> {
    let snapshot = state
        .files
        .collapse_directory_tree(&session_id)
        .map_err(|error| error.to_string())?;
    let _ = app.emit(DIRECTORY_TREE_UPDATED_EVENT, snapshot.clone());
    Ok(snapshot)
}

#[tauri::command]
pub(crate) async fn set_active_directory(
    session_id: String,
    directory: PathBuf,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let files = state.files.clone();
    let library = state.library.clone();
    let (session_id, directory) = tauri::async_runtime::spawn_blocking(move || {
        let root = files
            .session_root(&session_id)
            .map_err(|error| error.to_string())?;
        if library
            .has_directory_snapshot(&root, &directory)
            .map_err(|error| error.to_string())?
        {
            return Ok((session_id, directory));
        }
        match files.session_directory(&session_id, Some(&directory)) {
            Ok(directory) => Ok((session_id, directory)),
            // An unreachable NAS is not evidence that the saved directory was
            // deleted. Keep selection and let the browse request show its error.
            Err(oxy_fs::FsError::Io(error)) if error.kind() != std::io::ErrorKind::NotFound => {
                Ok((session_id, directory))
            }
            Err(error) => Err(error.to_string()),
        }
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
    let library = state.library.clone();
    let (tree, root, resolved_directory) = tauri::async_runtime::spawn_blocking(move || {
        let root = files
            .session_root(&session_id)
            .map_err(|error| error.to_string())?;
        let resolved_directory = files
            .session_directory(&session_id, directory.as_deref())
            .map_err(|error| error.to_string())?;
        library
            .invalidate_directory_snapshots(&resolved_directory)
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
            FileOperation::Trash { paths } | FileOperation::DeletePermanently { paths } => {
                for path in paths {
                    library
                        .remove_asset_tag_state(path)
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
        oxy_fs::open_in_file_manager(&path).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
