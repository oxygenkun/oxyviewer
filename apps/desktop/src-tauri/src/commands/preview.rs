use crate::{
    jobs::preview::{PreviewIdentity, PreviewRequest},
    state::AppState,
};
use oxy_domain::{
    AssetKind, CacheSettings, CacheSettingsUpdate, HeifCapabilities, HeifDecodeSession,
    HeifDecodeStatus, HeifDiagnostics, PreviewOmittedPolicy, PreviewPriority, PreviewResult,
    PreviewScheduleIntent, RenderLevel, SchedulePlacement,
};
use std::path::PathBuf;
use tauri::{Emitter, State};

#[tauri::command]
pub(crate) async fn get_preview(
    request_id: String,
    path: PathBuf,
    level: RenderLevel,
    priority: PreviewPriority,
    queue_order: Option<usize>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<oxy_domain::ImageProjection, String> {
    let files = state.files.clone();
    let preview_queue = state.preview_queue.clone();
    let preview_dir = state.cache.preview_dir();
    let cache = state.cache.clone();
    let projection = tauri::async_runtime::spawn_blocking(move || {
        let asset = files.get_asset(&path).map_err(|error| error.to_string())?;
        let (projection, receiver) = preview_queue.request(
            &app,
            PreviewRequest {
                request_id,
                path: asset.path.clone(),
                preview_dir,
                kind: asset.kind,
                size_bytes: asset.size_bytes,
                modified_at_ms: asset.modified_at_ms,
                level,
                priority,
                queue_order: queue_order.unwrap_or_default(),
            },
        )?;
        let _ = app.emit(
            crate::jobs::preview::IMAGE_PROJECTION_UPDATED_EVENT,
            projection,
        );
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
pub(crate) async fn reprioritize_preview(
    path: PathBuf,
    level: RenderLevel,
    request_id: String,
    priority: PreviewPriority,
    queue_order: Option<usize>,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let files = state.files.clone();
    let preview_queue = state.preview_queue.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let asset = files.get_asset(&path).map_err(|error| error.to_string())?;
        let identity = PreviewIdentity {
            path: asset.path,
            level,
        };
        Ok(preview_queue.reprioritize_pending(
            identity,
            &request_id,
            priority,
            queue_order.unwrap_or_default(),
        ))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn cancel_preview_request(
    path: PathBuf,
    level: RenderLevel,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let files = state.files.clone();
    let preview_queue = state.preview_queue.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let asset = files.get_asset(&path).map_err(|error| error.to_string())?;
        Ok(preview_queue.cancel_request(
            PreviewIdentity {
                path: asset.path,
                level,
            },
            &request_id,
        ))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn reconcile_preview_schedule(
    scope_id: String,
    epoch: u64,
    intents: Vec<PreviewScheduleIntent>,
    omitted_policy: PreviewOmittedPolicy,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let preview_queue = state.preview_queue.clone();
    tauri::async_runtime::spawn_blocking(move || {
        preview_queue.reconcile_schedule(scope_id, epoch, intents, omitted_policy)
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
    let preview_queue = state.preview_queue.clone();
    tauri::async_runtime::spawn_blocking(move || {
        preview_queue.upsert_schedule(
            scope_id,
            epoch,
            crate::jobs::preview::PreviewScheduleKey { path, level },
            priority,
            placement,
        )
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
    let preview_queue = state.preview_queue.clone();
    tauri::async_runtime::spawn_blocking(move || {
        preview_queue.release_schedule(
            &scope_id,
            epoch,
            &crate::jobs::preview::PreviewScheduleKey { path, level },
        )
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
pub(crate) fn get_heif_capabilities(state: State<'_, AppState>) -> Vec<HeifCapabilities> {
    state.heif.capabilities()
}

#[tauri::command]
pub(crate) fn get_heif_diagnostics(state: State<'_, AppState>) -> Option<HeifDiagnostics> {
    state.heif.diagnostics()
}

#[tauri::command]
pub(crate) async fn get_cached_heif_full(
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
pub(crate) async fn start_heif_decode(
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
pub(crate) fn cancel_heif_decode(session_id: String, state: State<'_, AppState>) -> bool {
    state.heif.cancel(&session_id)
}
