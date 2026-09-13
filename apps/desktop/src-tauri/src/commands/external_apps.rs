use crate::state::AppState;
use oxy_domain::{ExternalAppSettings, ExternalOpenResult};
use std::{path::PathBuf, sync::Arc};
use tauri::State;

#[tauri::command]
pub(crate) fn get_external_app_settings(
    state: State<'_, AppState>,
) -> Result<ExternalAppSettings, String> {
    state.external_apps.get()
}

#[tauri::command]
pub(crate) async fn update_external_app_settings(
    settings: ExternalAppSettings,
    state: State<'_, AppState>,
) -> Result<ExternalAppSettings, String> {
    let manager = Arc::clone(&state.external_apps);
    tauri::async_runtime::spawn_blocking(move || manager.update(settings))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn open_asset_with_application(
    path: PathBuf,
    app_id: String,
    state: State<'_, AppState>,
) -> Result<ExternalOpenResult, String> {
    let app = state.external_apps.application(&app_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        oxy_fs::external_apps::open_with_application(&app.executable_path, &path)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn open_asset_with_system_dialog(
    path: PathBuf,
) -> Result<ExternalOpenResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        oxy_fs::external_apps::open_with_system_dialog(&path).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
