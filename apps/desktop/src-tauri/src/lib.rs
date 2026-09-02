use oxy_domain::{
    AssetDetails, AssetKind, AssetQuery, AssetSummary, DirectorySummary, FileOperation,
    FileOperationResult, FolderSession, HeifCapabilities, HeifDecodeSession, HeifDecodeStatus,
    HeifDiagnostics, JobId, JobPriority, MetadataPatch, Page, PerfScenario, PreviewMode,
    PreviewPriority, PreviewResult,
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
        if query.minimum_rating.is_some() || query.color_label.is_some() {
            let mut assets = files
                .list_asset_candidates(&session_id, directory.as_deref())
                .map_err(|error| error.to_string())?;
            oxy_metadata::enrich_summaries(&mut assets).map_err(|error| error.to_string())?;
            Ok(oxy_fs::page_assets(&assets, &query, cursor.unwrap_or(0)))
        } else {
            files
                .list_assets(&session_id, directory.as_deref(), &query, cursor)
                .map_err(|error| error.to_string())
        }
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
async fn get_asset_details(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<AssetDetails, String> {
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let asset = files.get_asset(&path).map_err(|error| error.to_string())?;
        let kind = asset.kind;
        let dimensions = oxy_media::dimensions(&path).ok();
        let display_dimensions = dimensions.map(|value| (value.width, value.height));
        let focus_info = oxy_metadata::read_focus_info(&path, display_dimensions)
            .ok()
            .flatten();
        let sidecar_path = asset.has_sidecar.then(|| oxy_fs::sidecar_path(&path));
        Ok(AssetDetails {
            asset,
            width: dimensions.map(|value| value.width),
            height: dimensions.map(|value| value.height),
            metadata: oxy_metadata::read_metadata(&path, kind)
                .map_err(|error| error.to_string())?,
            sidecar_path,
            focus_info,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_preview(
    path: PathBuf,
    mode: PreviewMode,
    max_size: Option<u32>,
    priority: PreviewPriority,
    state: State<'_, AppState>,
) -> Result<PreviewResult, String> {
    let asset = state
        .files
        .get_asset(&path)
        .map_err(|error| error.to_string())?;
    // Raster formats the web view renders directly are served as originals;
    // every format that needs decoding funnels through the unified dispatcher.
    if !oxy_media::needs_decode(asset.kind) {
        return oxy_media::original(path).map_err(|error| error.to_string());
    }
    let preview_dir = state.preview_dir.clone();
    let max_size = max_size.unwrap_or(4_096).clamp(128, 8_192);
    let decode_priority = oxy_media::decode_priority_for(priority);
    let kind = asset.kind;
    tauri::async_runtime::spawn_blocking(move || {
        oxy_media::preview(&path, &preview_dir, mode, max_size, decode_priority, kind)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn execute_file_operation(operation: FileOperation) -> Result<FileOperationResult, String> {
    oxy_fs::execute_file_operation(&operation).map_err(|error| error.to_string())
}

#[tauri::command]
async fn patch_metadata(
    paths: Vec<PathBuf>,
    patch: MetadataPatch,
    state: State<'_, AppState>,
) -> Result<JobId, String> {
    let ticket = state.jobs.register(JobPriority::SelectedMetadata);
    let job_id = ticket.id.clone();
    let files = state.files.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        for path in paths {
            let asset = files.get_asset(&path).map_err(|error| error.to_string())?;
            oxy_metadata::patch_metadata(&path, asset.kind, &patch)
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
    Ok(job_id)
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
            remove_library_root,
            list_library_roots,
            cancel_job,
            get_heif_capabilities,
            get_heif_diagnostics,
            start_heif_decode,
            cancel_heif_decode,
            get_perf_scenario,
            write_perf_report
        ])
        .run(tauri::generate_context!())
        .expect("error while running OxyViewer");
}
