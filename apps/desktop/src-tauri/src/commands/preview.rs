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
    rank: Option<u32>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<oxy_domain::ImageProjection, String> {
    let preview_queue = state.preview_queue.clone();
    let selection = (level == RenderLevel::Full).then(|| preview_queue.select_full(&path));
    let preview_dir = state.cache.preview_dir();
    let projection = tauri::async_runtime::spawn_blocking(move || {
        let (projection, receiver) = preview_queue.request(
            &app,
            PreviewRequest {
                selection,
                request_id,
                path,
                preview_dir,
                level,
                priority,
                rank: rank.unwrap_or_default(),
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
    if projection.result.is_none() {
        return Err("ready image projection has no artifact".into());
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
    let preview_queue = state.preview_queue.clone();
    tauri::async_runtime::spawn_blocking(move || {
        Ok(preview_queue.cancel_request(PreviewIdentity { path, level }, &request_id))
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
    let selection = state.preview_queue.select_full(&path);
    let asset = state
        .files
        .get_asset(&path)
        .map_err(|error| error.to_string())?;
    if asset.kind != AssetKind::Heif {
        return Err("full-resolution HEIF presentation requires a HEIF asset".into());
    }
    let service = state.heif.clone();
    let preview_dir = state.cache.preview_dir();
    let lookup_queue = state.preview_queue.clone();
    let lookup_path = path.clone();
    let lookup_dir = preview_dir.clone();
    let (cached, use_artifact) = tauri::async_runtime::spawn_blocking(move || {
        let cached = lookup_queue.cached_heif_full_projection(
            &lookup_path,
            &lookup_dir,
            display_sharpening,
        )?;
        let use_artifact = if cached.is_some() || display_sharpening {
            false
        } else {
            oxy_media::full_uses_artifact(&lookup_path, &lookup_dir, false)
                .map_err(|error| error.to_string())?
        };
        Ok::<_, String>((cached, use_artifact))
    })
    .await
    .map_err(|error| error.to_string())??;
    state
        .preview_queue
        .check_full_selection(selection, &request_id)?;
    if let Some(projection) = cached {
        return Ok(HeifFullPresentation::Artifact {
            projection: Box::new(projection),
        });
    }
    if use_artifact {
        let preview_queue = state.preview_queue.clone();
        let projection = tauri::async_runtime::spawn_blocking(move || {
            let projection = resolve_heif_full_projection(
                &app,
                &preview_queue,
                asset,
                preview_dir,
                request_id,
                selection,
            )?;
            projection
                .result
                .as_ref()
                .ok_or_else(|| "ready HEIF full projection has no artifact".to_owned())?;
            Ok::<_, String>(projection)
        })
        .await
        .map_err(|error| error.to_string())??;
        return Ok(HeifFullPresentation::Artifact {
            projection: Box::new(projection),
        });
    }
    let cache = state.cache.clone();
    let preview_queue = state.preview_queue.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    state.preview_queue.enqueue_full_session(
        request_id,
        path.clone(),
        selection,
        move |cancellation| {
            let worker_session = match service.begin_cancellable(
                &path,
                &preview_dir,
                generation,
                display_sharpening,
                cancellation,
            ) {
                Ok(session) => session,
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    return;
                }
            };
            if sender.send(Ok(worker_session.clone())).is_err() {
                service.cancel(&worker_session.id);
                return;
            }
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
                // Lookup only: a cancelled dwell/cache failure must not trigger a
                // second source decode via the generic unsharpened preview queue.
                match preview_queue.cached_heif_full_projection(
                    &asset.path,
                    &preview_dir,
                    display_sharpening,
                ) {
                    Ok(Some(projection)) => {
                        if let Some(artifact) = projection.result {
                            cache.schedule_prune(Some(artifact.path));
                            if let Some(resource) = artifact.resource {
                                oxy_media::shared_resource_registry()
                                    .release(&resource.resource_id);
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("failed to inspect HEIF full cache: {error}"),
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
        },
    )?;
    let session = tauri::async_runtime::spawn_blocking(move || {
        receiver
            .recv()
            .map_err(|_| "full session cancelled before starting".to_owned())?
    })
    .await
    .map_err(|error| error.to_string())??;
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
    selection: u64,
) -> Result<ImageProjection, String> {
    let (loading, receiver) = preview_queue.request(
        app,
        PreviewRequest {
            selection: Some(selection),
            request_id,
            path: asset.path,
            preview_dir,
            level: RenderLevel::Full,
            priority: PreviewPriority::Loupe,
            rank: 0,
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
