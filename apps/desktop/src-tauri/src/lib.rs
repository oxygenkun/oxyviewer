mod cache;
mod directory_tree_queue;
mod exiftool;
mod metadata_queue;
mod preview_queue;

use oxy_domain::{
    AssetDetailsResult, AssetKind, AssetQuery, AssetSummary, CacheSettings, CacheSettingsUpdate,
    DirectorySearchMatch, DirectoryTreeSnapshot, EditableMetadata, FileOperation,
    FileOperationResult, FolderSession, HeifCapabilities, HeifDecodeSession, HeifDecodeStatus,
    HeifDiagnostics, JobId, JobPriority, LibraryIndexUpdate, MetadataCapability, MetadataPatch,
    MetadataProjection, MetadataProvider, MetadataRequestPriority, Page, PerfScenario,
    PreviewPriority, PreviewResult, RenderLevel,
};
use oxy_fs::FsCatalog;
use oxy_library::Library;
use oxy_runtime::JobRegistry;
use std::{path::PathBuf, sync::Arc};
use tauri::{Emitter, Manager, State, http};

use directory_tree_queue::{DIRECTORY_TREE_UPDATED_EVENT, DirectoryTreeQueue};

struct AppState {
    files: Arc<FsCatalog>,
    jobs: JobRegistry,
    library: Arc<Library>,
    cache: Arc<cache::CacheManager>,
    heif: Arc<oxy_media::HeifDecodeService>,
    metadata: oxy_metadata::MetadataFacade,
    metadata_queue: metadata_queue::MetadataQueue,
    preview_queue: preview_queue::PreviewQueue,
    directory_tree_queue: DirectoryTreeQueue,
    metadata_provider: Arc<exiftool::ProviderManager>,
}

const LIBRARY_INDEX_UPDATED_EVENT: &str = "library-index-updated";

fn schedule_library_index(app: tauri::AppHandle, library: Arc<Library>, root: PathBuf) {
    tauri::async_runtime::spawn_blocking(move || match library.index_root(&root) {
        Ok(Some(stats)) => {
            let _ = app.emit(
                LIBRARY_INDEX_UPDATED_EVENT,
                LibraryIndexUpdate {
                    root_path: root,
                    asset_count: stats.asset_count,
                    directory_count: stats.directory_count,
                },
            );
        }
        Ok(None) => {}
        Err(error) => eprintln!("library indexing failed for {}: {error}", root.display()),
    });
}

#[tauri::command]
fn open_folder(
    path: PathBuf,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<FolderSession, String> {
    let session = state
        .files
        .open_folder(path)
        .map_err(|error| error.to_string())?;
    if state
        .library
        .contains_root(&session.root_path)
        .unwrap_or(false)
    {
        schedule_library_index(app, state.library.clone(), session.root_path.clone());
    }
    Ok(session)
}

#[tauri::command]
async fn list_assets(
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
async fn get_directory_tree(
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
fn set_directory_expanded(
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
fn set_active_directory(
    session_id: String,
    directory: PathBuf,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let directory = state
        .files
        .known_directory_tree_path(&session_id, &directory)
        .map_err(|error| error.to_string())?;
    state.directory_tree_queue.set_active(session_id, directory);
    Ok(())
}

#[tauri::command]
async fn search_directories(
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
async fn refresh_directory(
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
    schedule_library_index(app, state.library.clone(), root);
    Ok(tree)
}

#[tauri::command]
async fn get_asset_details(
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

/// The native engine reads embedded metadata; sidecars override it. ExifTool
/// remains an optional write capability for explicit embedded synchronization.
fn metadata_for_details(
    _kind: AssetKind,
    has_sidecar: bool,
    result: Result<EditableMetadata, oxy_metadata::MetadataError>,
) -> (EditableMetadata, MetadataCapability) {
    let provider = if has_sidecar {
        MetadataProvider::Sidecar
    } else {
        MetadataProvider::Native
    };
    match result {
        Ok(metadata) => (
            metadata,
            MetadataCapability {
                provider,
                readable: true,
                writable: true,
                detail: None,
            },
        ),
        Err(error) => (
            EditableMetadata::default(),
            MetadataCapability {
                provider,
                readable: false,
                writable: true,
                detail: Some(error.to_string()),
            },
        ),
    }
}

#[tauri::command]
async fn request_metadata(
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
async fn get_preview(
    path: PathBuf,
    level: RenderLevel,
    priority: PreviewPriority,
    queue_order: Option<usize>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<oxy_domain::ImageProjection, String> {
    let asset = state
        .files
        .get_asset(&path)
        .map_err(|error| error.to_string())?;
    let cache = state.cache.clone();
    let (projection, receiver) = state.preview_queue.request(
        &app,
        preview_queue::PreviewRequest {
            path: asset.path.clone(),
            preview_dir: state.cache.preview_dir(),
            kind: asset.kind,
            size_bytes: asset.size_bytes,
            modified_at_ms: asset.modified_at_ms,
            level,
            priority,
            queue_order: queue_order.unwrap_or_default(),
        },
    )?;
    let _ = app.emit(preview_queue::IMAGE_PROJECTION_UPDATED_EVENT, projection);
    let projection = tauri::async_runtime::spawn_blocking(move || {
        receiver
            .recv()
            .map_err(|error| format!("preview queue stopped: {error}"))?
    })
    .await
    .map_err(|error| error.to_string())??;
    let protected_path = projection
        .result
        .as_ref()
        .map(|result| result.path.clone())
        .ok_or_else(|| "ready image projection has no artifact".to_owned())?;
    cache.mark_used(&protected_path);
    if cache.try_start_prune() {
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = cache.prune_after_write(&protected_path) {
                eprintln!("preview cache pruning failed: {error}");
            }
            cache.finish_prune();
        });
    }
    Ok(projection)
}

#[tauri::command]
async fn get_cache_settings(state: State<'_, AppState>) -> Result<CacheSettings, String> {
    let cache = state.cache.clone();
    tauri::async_runtime::spawn_blocking(move || cache.settings())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn update_cache_settings(
    update: CacheSettingsUpdate,
    state: State<'_, AppState>,
) -> Result<CacheSettings, String> {
    let cache = state.cache.clone();
    tauri::async_runtime::spawn_blocking(move || cache.update(update))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn clear_preview_cache(state: State<'_, AppState>) -> Result<CacheSettings, String> {
    let cache = state.cache.clone();
    let preview_queue = state.preview_queue.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let settings = cache.clear()?;
        preview_queue.invalidate_all();
        Ok(settings)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn execute_file_operation(operation: FileOperation) -> Result<FileOperationResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        oxy_fs::execute_file_operation(&operation).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn open_in_file_manager(path: PathBuf) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        oxy_fs::open_in_file_manager(&path).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn patch_metadata(
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
async fn sync_metadata_to_embedded(
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
async fn get_exiftool_status(
    state: State<'_, AppState>,
) -> Result<exiftool::ExiftoolStatus, String> {
    let provider = state.metadata_provider.clone();
    tauri::async_runtime::spawn_blocking(move || provider.status())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn configure_exiftool(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<exiftool::ExiftoolStatus, String> {
    let provider = state.metadata_provider.clone();
    tauri::async_runtime::spawn_blocking(move || provider.set_user_executable(path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn install_exiftool(state: State<'_, AppState>) -> Result<exiftool::ExiftoolStatus, String> {
    let provider = state.metadata_provider.clone();
    tauri::async_runtime::spawn_blocking(move || provider.install_managed())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn add_library_root(
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
fn remove_library_root(path: PathBuf, state: State<'_, AppState>) -> Result<Vec<PathBuf>, String> {
    state
        .library
        .remove_root(&path)
        .map_err(|error| error.to_string())?;
    state.library.roots().map_err(|error| error.to_string())
}

#[tauri::command]
fn list_library_roots(state: State<'_, AppState>) -> Result<Vec<PathBuf>, String> {
    state.library.roots().map_err(|error| error.to_string())
}

#[tauri::command]
fn reorder_library_roots(
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
fn cancel_job(job_id: String, state: State<'_, AppState>) -> bool {
    state.jobs.cancel(&job_id)
}

#[tauri::command]
fn get_heif_capabilities(state: State<'_, AppState>) -> Vec<HeifCapabilities> {
    state.heif.capabilities()
}

#[tauri::command]
fn get_heif_diagnostics(state: State<'_, AppState>) -> Option<HeifDiagnostics> {
    state.heif.diagnostics()
}

#[tauri::command]
async fn get_cached_heif_full(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<Option<PreviewResult>, String> {
    let asset = state
        .files
        .get_asset(&path)
        .map_err(|error| error.to_string())?;
    if asset.kind != AssetKind::Heif {
        return Err("full-resolution HEIF cache lookup requires a HEIF asset".into());
    }
    let preview_dir = state.cache.preview_dir();
    let cache = state.cache.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        oxy_media::cached_heif_session(&path, &preview_dir).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;
    if let Some(result) = &result {
        cache.mark_used(&result.path);
    }
    Ok(result)
}

#[tauri::command]
async fn start_heif_decode(
    path: PathBuf,
    generation: u64,
    hardware_acceleration: bool,
    display_sharpening: bool,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<HeifDecodeSession, String> {
    let asset = state
        .files
        .get_asset(&path)
        .map_err(|error| error.to_string())?;
    if asset.kind != AssetKind::Heif {
        return Err("full-resolution HEIF sessions require a HEIF asset".into());
    }
    let service = state.heif.clone();
    let session = service
        .begin(&path, generation, hardware_acceleration, display_sharpening)
        .map_err(|error| error.to_string())?;
    let worker_session = session.clone();
    let preview_dir = state.cache.preview_dir();
    let cache = state.cache.clone();
    let fallback_status = (session.status == HeifDecodeStatus::CompatibilityFallback).then(|| {
        oxy_media::HeifDecodeService::status_event(
            &session,
            HeifDecodeStatus::CompatibilityFallback,
            None,
            Some("native hardware decoder unavailable; using compatibility mode".into()),
        )
    });
    if let Some(status) = fallback_status {
        let _ = app.emit("heif-decode-status", status);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let cache_source_path = path.clone();
        let result = service.decode(
            &worker_session,
            path,
            &preview_dir,
            hardware_acceleration,
            |tile| {
                let _ = app.emit("heif-tile-ready", tile);
            },
            |diagnostics| {
                let event = oxy_media::HeifDecodeService::status_event(
                    &worker_session,
                    HeifDecodeStatus::Complete,
                    Some(diagnostics.clone()),
                    None,
                );
                let _ = app.emit("heif-decode-status", event);
            },
        );
        if result.is_ok()
            && let Ok(Some(cached)) =
                oxy_media::cached_heif_session(&cache_source_path, &preview_dir)
        {
            cache.mark_used(&cached.path);
            if cache.try_start_prune() {
                if let Err(error) = cache.prune_after_write(&cached.path) {
                    eprintln!("preview cache pruning failed: {error}");
                }
                cache.finish_prune();
            }
            let _ = app.emit("heif-cache-ready", worker_session.clone());
        }
        let event = match result {
            Ok(_) => None,
            Err(oxy_media::MediaError::Cancelled) => {
                Some(oxy_media::HeifDecodeService::status_event(
                    &worker_session,
                    HeifDecodeStatus::Cancelled,
                    None,
                    None,
                ))
            }
            Err(error) => Some(oxy_media::HeifDecodeService::status_event(
                &worker_session,
                HeifDecodeStatus::Failed,
                None,
                Some(error.to_string()),
            )),
        };
        if let Some(event) = event {
            let _ = app.emit("heif-decode-status", event);
        }
    });
    Ok(session)
}

#[tauri::command]
fn cancel_heif_decode(session_id: String, state: State<'_, AppState>) -> bool {
    state.heif.cancel(&session_id)
}

/// Returns the performance scenario injected via `OXY_PERF_SCENARIO`, if any.
/// Used only by the end-to-end performance harness (docs/PERF_E2E.md).
#[tauri::command]
fn get_perf_scenario() -> Option<PerfScenario> {
    let raw = std::env::var("OXY_PERF_SCENARIO").ok()?;
    serde_json::from_str(&raw).ok()
}

/// Writes the performance harness report JSON to a runner-owned path.
#[tauri::command]
fn write_perf_report(path: PathBuf, contents: String) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, contents).map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let heif = Arc::new(oxy_media::HeifDecodeService::default());
    let protocol_heif = heif.clone();
    tauri::Builder::default()
        .register_uri_scheme_protocol("oxy-media", move |_context, request| {
            let parts = request
                .uri()
                .path()
                .trim_start_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            let tile = if parts.len() == 5 && parts[0] == "tile" {
                let generation = parts[2].parse().ok();
                let x = parts[3].parse().ok();
                let y = parts[4].parse().ok();
                match (generation, x, y) {
                    (Some(generation), Some(x), Some(y)) => {
                        protocol_heif.tile(parts[1], generation, x, y)
                    }
                    _ => None,
                }
            } else {
                None
            };
            match tile {
                Some(tile) => {
                    let (content_type, body) = match tile.encoded_jpeg {
                        Some(jpeg) => ("image/jpeg", jpeg.to_vec()),
                        None => ("application/octet-stream", tile.rgba.to_vec()),
                    };
                    http::Response::builder()
                        .status(http::StatusCode::OK)
                        .header(http::header::CONTENT_TYPE, content_type)
                        .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                        .header("x-oxy-width", tile.width)
                        .header("x-oxy-height", tile.height)
                        .header("x-oxy-stride", tile.stride)
                        .body(body)
                        .expect("valid tile protocol response")
                }
                None => http::Response::builder()
                    .status(http::StatusCode::NOT_FOUND)
                    .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .body(Vec::new())
                    .expect("valid missing tile response"),
            }
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
            ) {
                window.app_handle().exit(0);
            }
        })
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            let preview_dir = app.path().app_cache_dir()?.join("previews");
            let cache = Arc::new(cache::CacheManager::load(
                preview_dir,
                data_dir.join("cache-settings.json"),
            )?);
            let library = Arc::new(Library::open(&data_dir.join("oxyviewer.sqlite"))?);
            let metadata_provider = Arc::new(exiftool::ProviderManager::load(data_dir));
            let files = Arc::new(FsCatalog::default());
            let directory_tree_queue = DirectoryTreeQueue::new(app.handle().clone(), files.clone());
            let metadata = metadata_provider.facade();
            let metadata_queue = metadata_queue::MetadataQueue::new(
                app.handle().clone(),
                files.clone(),
                metadata.clone(),
                library.clone(),
            );
            let preview_queue =
                preview_queue::PreviewQueue::new(app.handle().clone(), library.clone());
            app.manage(AppState {
                files,
                jobs: JobRegistry::default(),
                library,
                cache,
                heif: heif.clone(),
                metadata,
                metadata_queue,
                preview_queue,
                directory_tree_queue,
                metadata_provider,
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_folder,
            list_assets,
            get_directory_tree,
            set_directory_expanded,
            set_active_directory,
            search_directories,
            refresh_directory,
            get_asset_details,
            request_metadata,
            get_preview,
            get_cache_settings,
            update_cache_settings,
            clear_preview_cache,
            patch_metadata,
            sync_metadata_to_embedded,
            get_exiftool_status,
            configure_exiftool,
            install_exiftool,
            execute_file_operation,
            open_in_file_manager,
            add_library_root,
            remove_library_root,
            list_library_roots,
            reorder_library_roots,
            cancel_job,
            get_heif_capabilities,
            get_heif_diagnostics,
            get_cached_heif_full,
            start_heif_decode,
            cancel_heif_decode,
            get_perf_scenario,
            write_perf_report
        ])
        .run(tauri::generate_context!())
        .expect("error while running OxyViewer");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_details_do_not_require_the_optional_exiftool_worker() {
        let (metadata, capability) = metadata_for_details(
            AssetKind::Heif,
            false,
            Err(oxy_metadata::MetadataError::EmbeddedWorkerUnavailable),
        );

        assert_eq!(metadata, EditableMetadata::default());
        assert_eq!(capability.provider, MetadataProvider::Native);
        assert!(!capability.readable);
        assert!(capability.writable);
        assert!(capability.detail.is_some());
    }

    #[test]
    fn metadata_read_error_does_not_masquerade_as_missing_provider() {
        let (_, capability) = metadata_for_details(
            AssetKind::Heif,
            false,
            Err(oxy_metadata::MetadataError::Read("invalid XMP".into())),
        );

        assert!(!capability.readable);
        assert!(capability.writable);
    }
}
