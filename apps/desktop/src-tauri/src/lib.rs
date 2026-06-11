use oxy_domain::{
    AssetDetails, AssetKind, AssetQuery, AssetSummary, DirectorySummary, EditableMetadata,
    FileOperation, FileOperationResult, FolderSession, JobId, JobPriority, Page, PreviewMode,
    PreviewResult,
};
use oxy_fs::FsCatalog;
use oxy_library::Library;
use oxy_runtime::JobRegistry;
use std::{path::PathBuf, sync::Arc};
use tauri::{Manager, State};

struct AppState {
    files: Arc<FsCatalog>,
    jobs: JobRegistry,
    library: Arc<Library>,
    preview_dir: PathBuf,
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
            if mode == PreviewMode::FullRaw {
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
            if mode == PreviewMode::FullRaw {
                return Err("fullRaw preview mode only supports RAW assets".into());
            }
            oxy_media::system_preview(&path, &preview_dir, max_size)
                .map_err(|error| error.to_string())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let preview_dir = app.path().app_cache_dir()?.join("previews");
            let library = Library::open(&data_dir.join("oxyviewer.sqlite"))?;
            app.manage(AppState {
                files: Arc::new(FsCatalog::default()),
                jobs: JobRegistry::default(),
                library: Arc::new(library),
                preview_dir,
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
            cancel_job
        ])
        .run(tauri::generate_context!())
        .expect("error while running OxyViewer");
}
