//! HEIF presentation policy and callbacks over the existing owned full queue.
use super::{PreparedPreview, ResourceService};
use crate::{
    events::CoreEvent,
    resources::{
        preview::PreviewRequest,
        types::{HeifFullPresentation, HeifStatusEvent},
    },
};
use oxy_media::types::{
    AssetKind, HeifDecodeSession, HeifDecodeStatus, HeifDiagnostics, PreviewPriority, RenderLevel,
};
use std::path::PathBuf;

pub struct HeifFullInput {
    pub request_id: String,
    pub path: PathBuf,
    pub generation: u64,
    pub display_sharpening: bool,
}

pub struct PreparedHeifFull {
    input: HeifFullInput,
    selection: u64,
}

impl ResourceService {
    /// Capture selection before dispatch; source inspection belongs to resolution.
    pub fn prepare_heif_full(&self, input: HeifFullInput) -> Result<PreparedHeifFull, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let selection = self.queue.select_full(&input.path);
        Ok(PreparedHeifFull { input, selection })
    }

    /// Run on a blocking executor. Native decode runs in the owned full queue.
    pub fn resolve_heif_full(
        &self,
        prepared: PreparedHeifFull,
    ) -> Result<HeifFullPresentation, String> {
        let PreparedHeifFull { input, selection } = prepared;
        let HeifFullInput {
            request_id,
            path,
            generation,
            display_sharpening,
        } = input;
        let asset = self.files.get_asset(&path).map_err(|e| e.to_string())?;
        if asset.kind != AssetKind::Heif {
            return Err("full-resolution HEIF presentation requires a HEIF asset".into());
        }
        let preview_dir = self.cache.preview_dir();
        let cached =
            self.queue
                .cached_heif_full_projection(&path, &preview_dir, display_sharpening)?;
        let use_artifact = if cached.is_some() || display_sharpening {
            false
        } else {
            oxy_media::full_uses_artifact(&path, &preview_dir, false).map_err(|e| e.to_string())?
        };
        self.queue.check_full_selection(selection, &request_id)?;
        if let Some(projection) = cached {
            return Ok(HeifFullPresentation::Artifact {
                projection: Box::new(projection),
            });
        }
        if use_artifact {
            let projection = self.resolve_preview(PreparedPreview {
                request: PreviewRequest {
                    selection: Some(selection),
                    request_id,
                    path: asset.path,
                    preview_dir,
                    level: RenderLevel::Full,
                    priority: PreviewPriority::Loupe,
                    rank: 0,
                },
                cross_folder: false,
            })?;
            return Ok(HeifFullPresentation::Artifact {
                projection: Box::new(projection),
            });
        }
        let service = self.heif.clone();
        let cache = self.cache.clone();
        let queue = self.queue.clone();
        let runtime = self.runtime.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        self.queue.enqueue_full_session(
            request_id,
            path.clone(),
            selection,
            move |cancellation| {
                let session = match service.begin_cancellable(
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
                if sender.send(Ok(session.clone())).is_err() {
                    service.cancel(&session.id);
                    return;
                }
                let result = service.decode(
                    &session,
                    &preview_dir,
                    |tile| {
                        let _ = runtime.publish(CoreEvent::HeifTilePublished(Box::new(tile)));
                    },
                    |diagnostics| {
                        let event = status_event(
                            &session,
                            HeifDecodeStatus::Complete,
                            Some(diagnostics.clone()),
                            None,
                        );
                        let _ = runtime.publish(CoreEvent::HeifStatusUpdated(event));
                    },
                );
                if result.is_ok() {
                    // Lookup only; never decode again after a cancelled dwell/cache failure.
                    match queue.cached_heif_full_projection(
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
                    Err(oxy_media::MediaError::Cancelled) => Some(status_event(
                        &session,
                        HeifDecodeStatus::Cancelled,
                        None,
                        None,
                    )),
                    Err(error) => Some(status_event(
                        &session,
                        HeifDecodeStatus::Failed,
                        None,
                        Some(error.to_string()),
                    )),
                };
                if let Some(event) = event {
                    let _ = runtime.publish(CoreEvent::HeifStatusUpdated(event));
                }
            },
        )?;
        let session = receiver
            .recv()
            .map_err(|_| "full session cancelled before starting".to_owned())??;
        Ok(HeifFullPresentation::Tiles { session })
    }
}

fn status_event(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppPaths, OxyApp};
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "requires OXY_HIF_FIXTURE and an installed native HEIF decoder"]
    fn real_heif_use_case_publishes_tiles_and_reuses_cached_artifact() {
        let path = PathBuf::from(std::env::var_os("OXY_HIF_FIXTURE").expect("set OXY_HIF_FIXTURE"));
        let directory = tempfile::tempdir().unwrap();
        let (app, events) = OxyApp::open(AppPaths {
            data_dir: directory.path().join("data"),
            preview_dir: directory.path().join("previews"),
        })
        .unwrap();
        let input = |id: &str| HeifFullInput {
            request_id: id.into(),
            path: path.clone(),
            generation: 77,
            display_sharpening: true,
        };
        let prepared = app
            .resource_service
            .prepare_heif_full(input("first"))
            .unwrap();
        let HeifFullPresentation::Tiles { session } =
            app.resource_service.resolve_heif_full(prepared).unwrap()
        else {
            panic!("an empty sharpened cache must start a native session");
        };
        let deadline = Instant::now() + Duration::from_secs(40);
        let mut tiles = 0;
        loop {
            match events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                CoreEvent::HeifTilePublished(tile) => {
                    assert_eq!(tile.session_id, session.id);
                    assert_eq!(tile.generation, 77);
                    assert!(tile.width > 0 && tile.height > 0);
                    tiles += 1;
                }
                CoreEvent::HeifStatusUpdated(status) if status.session_id == session.id => {
                    assert_eq!(
                        status.status,
                        HeifDecodeStatus::Complete,
                        "{:?}",
                        status.message
                    );
                    break;
                }
                _ => {}
            }
        }
        assert!(tiles > 0);
        let projection = loop {
            if let Some(projection) = app
                .resource_service
                .queue
                .cached_heif_full_projection(&path, &app.cache.preview_dir(), true)
                .unwrap()
            {
                break projection;
            }
            assert!(
                Instant::now() < deadline,
                "completed session did not persist its artifact"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        if let Some(resource) = projection.result.and_then(|artifact| artifact.resource) {
            app.media_resources.release(&resource.resource_id);
        }
        let prepared = app
            .resource_service
            .prepare_heif_full(input("cached"))
            .unwrap();
        let HeifFullPresentation::Artifact { projection } =
            app.resource_service.resolve_heif_full(prepared).unwrap()
        else {
            panic!("the completed session must be reused as an artifact");
        };
        let artifact = projection.result.expect("cached full result");
        assert!(artifact.path.is_file());
        if let Some(resource) = artifact.resource {
            app.media_resources.release(&resource.resource_id);
        }
        assert!(app.shutdown(Duration::from_secs(5)));
    }
}
