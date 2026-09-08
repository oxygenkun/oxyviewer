use crate::{
    jobs::preview::{PreviewIdentity, PreviewRequest},
    state::AppState,
};
use oxy_domain::{
    AssetKind, AssetSummary, CacheSettings, CacheSettingsUpdate, HeifCapabilities,
    HeifDecodeSession, HeifDecodeStatus, HeifDiagnostics, HeifFullPresentation, HeifStatusEvent,
    HeifTileReady, ImageProjection, PreviewOmittedPolicy, PreviewPriority, PreviewScheduleIntent,
    RenderLevel, SchedulePlacement,
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
    let result = projection
        .result
        .as_ref()
        .ok_or_else(|| "ready image projection has no artifact".to_owned())?;
    if result.persistence == Some(oxy_domain::MediaPersistence::Persisted) {
        let protected_path = result.path.clone();
        cache.mark_used(&protected_path);
        if cache.try_start_prune() {
            tauri::async_runtime::spawn_blocking(move || {
                loop {
                    if let Err(error) = cache.prune_after_write(&protected_path) {
                        eprintln!("preview cache pruning failed: {error}");
                    }
                    if !cache.finish_prune() {
                        break;
                    }
                }
            });
        }
    }
    Ok(projection)
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
pub(crate) async fn start_heif_full(
    request_id: String,
    path: PathBuf,
    generation: u64,
    display_sharpening: bool,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<HeifFullPresentation, String> {
    let asset = state
        .files
        .get_asset(&path)
        .map_err(|error| error.to_string())?;
    if asset.kind != AssetKind::Heif {
        return Err("full-resolution HEIF presentation requires a HEIF asset".into());
    }
    let service = state.heif.clone();
    let preview_dir = state.cache.preview_dir();
    let use_artifact = oxy_media::full_uses_artifact(&path, &preview_dir, display_sharpening)
        .map_err(|error| error.to_string())?;
    if use_artifact {
        let preview_queue = state.preview_queue.clone();
        let cache = state.cache.clone();
        let projection = tauri::async_runtime::spawn_blocking(move || {
            let projection =
                resolve_heif_full_projection(&app, &preview_queue, asset, preview_dir, request_id)?;
            let artifact = projection
                .result
                .as_ref()
                .ok_or_else(|| "ready HEIF full projection has no artifact".to_owned())?;
            cache.mark_used(&artifact.path);
            Ok::<_, String>(projection)
        })
        .await
        .map_err(|error| error.to_string())??;
        return Ok(HeifFullPresentation::Artifact {
            projection: Box::new(projection),
        });
    }
    let session = service
        .begin(&path, &preview_dir, generation, display_sharpening)
        .map_err(|error| error.to_string())?;
    let worker_session = session.clone();
    let cache = state.cache.clone();
    let preview_queue = state.preview_queue.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = service.decode(
            &worker_session,
            &preview_dir,
            |tile| {
                let _ = app.emit("heif-tile-ready", heif_tile_event(tile));
            },
            |diagnostics| {
                let event = heif_status_event(
                    &worker_session,
                    HeifDecodeStatus::Complete,
                    Some(diagnostics.clone()),
                    None,
                );
                let _ = app.emit("heif-decode-status", event);
            },
        );
        if result.is_ok() {
            match resolve_heif_full_projection(&app, &preview_queue, asset, preview_dir, request_id)
            {
                Ok(projection) => {
                    if let Some(artifact) = projection.result {
                        cache.mark_used(&artifact.path);
                        if cache.try_start_prune() {
                            loop {
                                if let Err(error) = cache.prune_after_write(&artifact.path) {
                                    eprintln!("preview cache pruning failed: {error}");
                                }
                                if !cache.finish_prune() {
                                    break;
                                }
                            }
                        }
                    }
                }
                Err(error) => eprintln!("failed to publish HEIF full projection: {error}"),
            }
        }
        let event = match result {
            Ok(_) => None,
            Err(oxy_media::MediaError::Cancelled) => Some(heif_status_event(
                &worker_session,
                HeifDecodeStatus::Cancelled,
                None,
                None,
            )),
            Err(error) => Some(heif_status_event(
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
    Ok(HeifFullPresentation::Tiles { session })
}

fn heif_tile_event(tile: oxy_media::HeifTilePublication) -> HeifTileReady {
    HeifTileReady {
        session_id: tile.session_id.clone(),
        generation: tile.generation,
        x: tile.x,
        y: tile.y,
        width: tile.width,
        height: tile.height,
        payload: tile.payload,
        url: format!(
            "oxy-media://localhost/tile/{}/{}/{}/{}",
            tile.session_id, tile.generation, tile.x, tile.y
        ),
    }
}

fn heif_status_event(
    session: &HeifDecodeSession,
    status: HeifDecodeStatus,
    diagnostics: Option<HeifDiagnostics>,
    message: Option<String>,
) -> HeifStatusEvent {
    HeifStatusEvent {
        session_id: session.id.clone(),
        generation: session.generation,
        status,
        diagnostics,
        message,
    }
}

fn resolve_heif_full_projection(
    app: &tauri::AppHandle,
    preview_queue: &crate::jobs::preview::PreviewQueue,
    asset: AssetSummary,
    preview_dir: PathBuf,
    request_id: String,
) -> Result<ImageProjection, String> {
    let (loading, receiver) = preview_queue.request(
        app,
        PreviewRequest {
            request_id,
            path: asset.path,
            preview_dir,
            kind: asset.kind,
            size_bytes: asset.size_bytes,
            modified_at_ms: asset.modified_at_ms,
            level: RenderLevel::Full,
            priority: PreviewPriority::Loupe,
            queue_order: 0,
        },
    )?;
    let _ = app.emit(
        crate::jobs::preview::IMAGE_PROJECTION_UPDATED_EVENT,
        loading,
    );
    receiver
        .recv()
        .map_err(|error| format!("preview queue stopped: {error}"))?
}

#[tauri::command]
pub(crate) fn cancel_heif_decode(session_id: String, state: State<'_, AppState>) -> bool {
    state.heif.cancel(&session_id)
}
