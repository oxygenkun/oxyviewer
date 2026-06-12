use oxy_domain::{
    AssetDetails, AssetKind, AssetQuery, AssetSummary, DirectorySummary, EditableMetadata,
    FileOperation, FileOperationResult, FolderSession, HeifCapabilities, HeifDecodeSession,
    HeifDecodeStatus, HeifDiagnostics, JobId, JobPriority, Page, PreviewMode, PreviewResult,
};
use oxy_fs::FsCatalog;
use oxy_library::Library;
use oxy_runtime::JobRegistry;
use std::{path::PathBuf, sync::Arc};
use tauri::{Emitter, Manager, State, http};

struct AppState {
    files: Arc<FsCatalog>,
    jobs: JobRegistry,
    library: Arc<Library>,
    preview_dir: PathBuf,
    heif: Arc<oxy_media::HeifDecodeService>,
}

#[tauri::command]
fn open_folder(path: PathBuf, state: State<'_, AppState>) -> Result<FolderSession, String> {
    state
        .files
        .open_folder(path)
        .map_err(|error| error.to_string())
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
    tauri::async_runtime::spawn_blocking(move || {
        files
            .list_assets(&session_id, directory.as_deref(), &query, cursor)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn list_directories(
    session_id: String,
    directory: Option<PathBuf>,
    state: State<'_, AppState>,
) -> Result<Vec<DirectorySummary>, String> {
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        files
            .list_directories(&session_id, directory.as_deref())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn refresh_directory(
    session_id: String,
    directory: Option<PathBuf>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .files
        .refresh_directory(&session_id, directory.as_deref())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_asset_details(path: PathBuf, state: State<'_, AppState>) -> Result<AssetDetails, String> {
    let asset = state
        .files
        .get_asset(&path)
        .map_err(|error| error.to_string())?;
    let dimensions = oxy_media::dimensions(&path).ok();
    let sidecar_path = asset.has_sidecar.then(|| oxy_fs::sidecar_path(&path));
    Ok(AssetDetails {
        asset,
        width: dimensions.map(|value| value.width),
        height: dimensions.map(|value| value.height),
        metadata: Default::default(),
        sidecar_path,
    })
}

#[tauri::command]
async fn get_preview(
    path: PathBuf,
    mode: PreviewMode,
    max_size: Option<u32>,
    state: State<'_, AppState>,
) -> Result<PreviewResult, String> {
    let asset = state
        .files
        .get_asset(&path)
        .map_err(|error| error.to_string())?;
    if !matches!(
        asset.kind,
        AssetKind::Raw | AssetKind::Heif | AssetKind::Tiff
    ) {
        return oxy_media::original(path).map_err(|error| error.to_string());
    }
    let preview_dir = state.preview_dir.clone();
    let max_size = max_size.unwrap_or(4_096).clamp(128, 8_192);
    tauri::async_runtime::spawn_blocking(move || {
        if asset.kind == AssetKind::Raw {
            if mode == PreviewMode::FullDetail {
                return oxy_media::raw_full(&path, &preview_dir).map_err(|error| error.to_string());
            }
            oxy_media::raw_preview(&path, &preview_dir, max_size)
                .map_err(|error| error.to_string())
                .or_else(|libraw_error| {
                    oxy_media::system_preview(&path, &preview_dir, max_size).map_err(
                        |system_error| format!("{libraw_error}; fallback failed: {system_error}"),
                    )
                })
        } else {
            if mode == PreviewMode::FullDetail && asset.kind == AssetKind::Heif {
                return oxy_media::heif_full(&path, &preview_dir)
                    .map_err(|error| error.to_string())
                    .or_else(|heif_error| {
                        eprintln!(
                            "full-detail libheif decode failed for {}: {heif_error}",
                            path.display()
                        );
                        oxy_media::heif_preview(&path, &preview_dir, 8_192).map_err(
                            |preview_error| {
                                format!("{heif_error}; preview fallback failed: {preview_error}")
                            },
                        )
                    });
            }
            if mode == PreviewMode::FullDetail {
                return Err("fullDetail preview mode only supports RAW and HEIF assets".into());
            }
            if asset.kind == AssetKind::Heif {
                oxy_media::heif_preview(&path, &preview_dir, max_size)
                    .map_err(|error| error.to_string())
            } else {
                oxy_media::system_preview(&path, &preview_dir, max_size)
                    .map_err(|error| error.to_string())
            }
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn execute_file_operation(operation: FileOperation) -> Result<FileOperationResult, String> {
    oxy_fs::execute_file_operation(&operation).map_err(|error| error.to_string())
}

#[tauri::command]
fn patch_metadata(
    paths: Vec<PathBuf>,
    metadata: EditableMetadata,
    state: State<'_, AppState>,
) -> Result<JobId, String> {
    let ticket = state.jobs.register(JobPriority::SelectedMetadata);
    for path in paths {
        let asset = state
            .files
            .get_asset(&path)
            .map_err(|error| error.to_string())?;
        oxy_metadata::write_metadata(&path, asset.kind, &metadata)
            .map_err(|error| error.to_string())?;
    }
    state.jobs.finish(&ticket.id);
    Ok(ticket.id)
}

#[tauri::command]
fn add_library_root(path: PathBuf, state: State<'_, AppState>) -> Result<Vec<PathBuf>, String> {
    state
        .library
        .add_root(&path)
        .map_err(|error| error.to_string())?;
    state.library.roots().map_err(|error| error.to_string())
}

#[tauri::command]
fn list_library_roots(state: State<'_, AppState>) -> Result<Vec<PathBuf>, String> {
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
async fn start_heif_decode(
    path: PathBuf,
    generation: u64,
    hardware_acceleration: bool,
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
        .begin(&path, generation, hardware_acceleration)
        .map_err(|error| error.to_string())?;
    let worker_session = session.clone();
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
        let result = service.decode(&worker_session, path, |tile| {
            let _ = app.emit("heif-tile-ready", tile);
        });
        let event = match result {
            Ok(diagnostics) => oxy_media::HeifDecodeService::status_event(
                &worker_session,
                HeifDecodeStatus::Complete,
                Some(diagnostics),
                None,
            ),
            Err(oxy_media::MediaError::Cancelled) => oxy_media::HeifDecodeService::status_event(
                &worker_session,
                HeifDecodeStatus::Cancelled,
                None,
                None,
            ),
            Err(error) => oxy_media::HeifDecodeService::status_event(
                &worker_session,
                HeifDecodeStatus::Failed,
                None,
                Some(error.to_string()),
            ),
        };
        let _ = app.emit("heif-decode-status", event);
    });
    Ok(session)
}

#[tauri::command]
fn cancel_heif_decode(session_id: String, state: State<'_, AppState>) -> bool {
    state.heif.cancel(&session_id)
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
                Some(tile) => http::Response::builder()
                    .status(http::StatusCode::OK)
                    .header(http::header::CONTENT_TYPE, "application/octet-stream")
                    .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .header("x-oxy-width", tile.width)
                    .header("x-oxy-height", tile.height)
                    .header("x-oxy-stride", tile.stride)
                    .body(tile.rgba.to_vec())
                    .expect("valid tile protocol response"),
                None => http::Response::builder()
                    .status(http::StatusCode::NOT_FOUND)
                    .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .body(Vec::new())
                    .expect("valid missing tile response"),
            }
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            let preview_dir = app.path().app_cache_dir()?.join("previews");
            let library = Library::open(&data_dir.join("oxyviewer.sqlite"))?;
            app.manage(AppState {
                files: Arc::new(FsCatalog::default()),
                jobs: JobRegistry::default(),
                library: Arc::new(library),
                preview_dir,
                heif: heif.clone(),
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_folder,
            list_assets,
            list_directories,
            refresh_directory,
            get_asset_details,
            get_preview,
            patch_metadata,
            execute_file_operation,
            add_library_root,
            list_library_roots,
            cancel_job,
            get_heif_capabilities,
            get_heif_diagnostics,
            start_heif_decode,
            cancel_heif_decode
        ])
        .run(tauri::generate_context!())
        .expect("error while running OxyViewer");
}
