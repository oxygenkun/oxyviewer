use crate::state::AppState;
use oxy_core::contracts::{
    CacheSettings, CacheSettingsUpdate, HeifCapabilities, HeifFullPresentation,
    PreviewOmittedPolicy, PreviewPriority, PreviewScheduleIntent, RenderLevel, SchedulePlacement,
};
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub(crate) async fn get_preview(
    request_id: String,
    path: PathBuf,
    level: RenderLevel,
    priority: PreviewPriority,
    rank: Option<u32>,
    cross_folder: Option<bool>,
    state: State<'_, AppState>,
) -> Result<oxy_core::contracts::ImageProjection, String> {
    let service = state.resource_service.clone();
    let prepared = service.prepare_preview(oxy_core::resources::service::PreviewInput {
        request_id,
        path,
        level,
        priority,
        rank: rank.unwrap_or_default(),
        cross_folder: cross_folder.unwrap_or(false),
    })?;
    tauri::async_runtime::spawn_blocking(move || service.resolve_preview(prepared))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) fn renew_media_resource(resource_id: String, state: State<'_, AppState>) -> bool {
    state.media_resources.renew(&resource_id)
}

#[tauri::command]
pub(crate) fn release_media_resource(resource_id: String, state: State<'_, AppState>) {
    state.media_resources.release(&resource_id);
}

#[tauri::command]
pub(crate) async fn cancel_preview_request(
    path: PathBuf,
    level: RenderLevel,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let service = state.resource_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.cancel_preview(path, level, &request_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn reconcile_preview_schedule(
    scope_id: String,
    epoch: u64,
    intents: Vec<PreviewScheduleIntent>,
    omitted_policy: PreviewOmittedPolicy,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let service = state.resource_service.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.reconcile_schedule(scope_id, epoch, intents, omitted_policy)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn upsert_preview_schedule(
    scope_id: String,
    epoch: u64,
    path: PathBuf,
    level: RenderLevel,
    priority: PreviewPriority,
    placement: SchedulePlacement,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let service = state.resource_service.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.upsert_schedule(scope_id, epoch, path, level, priority, placement)
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn release_preview_schedule(
    scope_id: String,
    epoch: u64,
    path: PathBuf,
    level: RenderLevel,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let service = state.resource_service.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.release_schedule(&scope_id, epoch, path, level)
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn get_cache_settings(
    state: State<'_, AppState>,
) -> Result<CacheSettings, String> {
    let cache = state.cache.clone();
    tauri::async_runtime::spawn_blocking(move || cache.settings())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn update_cache_settings(
    update: CacheSettingsUpdate,
    state: State<'_, AppState>,
) -> Result<CacheSettings, String> {
    let cache = state.cache.clone();
    tauri::async_runtime::spawn_blocking(move || cache.update(update))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn clear_preview_cache(
    state: State<'_, AppState>,
) -> Result<CacheSettings, String> {
    let service = state.resource_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.clear_preview_cache())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) fn get_heif_capabilities(state: State<'_, AppState>) -> Vec<HeifCapabilities> {
    state.resource_service.heif_capabilities()
}

#[tauri::command]
pub(crate) async fn start_heif_full(
    request_id: String,
    path: PathBuf,
    generation: u64,
    display_sharpening: bool,
    state: State<'_, AppState>,
) -> Result<HeifFullPresentation, String> {
    let service = state.resource_service.clone();
    let prepared = service.prepare_heif_full(oxy_core::resources::service::HeifFullInput {
        request_id,
        path,
        generation,
        display_sharpening,
    })?;
    tauri::async_runtime::spawn_blocking(move || service.resolve_heif_full(prepared))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) fn cancel_heif_decode(session_id: String, state: State<'_, AppState>) -> bool {
    state.resource_service.cancel_heif(&session_id)
}
