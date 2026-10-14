//! Application preview admission, delivery and explicit cache maintenance.
use super::{
    cache::CacheManager,
    preview::{PreviewQueue, PreviewRequest},
    types::{CacheSettings, ImageProjection},
};
use crate::{events::CoreEvent, runtime::CoreRuntime};
use oxy_media::types::{PreviewPriority, RenderLevel};
use std::{path::PathBuf, sync::Arc};

pub struct ResourceService {
    pub(crate) runtime: CoreRuntime,
    pub(crate) queue: PreviewQueue,
    pub(crate) cache: Arc<CacheManager>,
}

pub struct PreviewInput {
    pub request_id: String,
    pub path: PathBuf,
    pub level: RenderLevel,
    pub priority: PreviewPriority,
    pub rank: u32,
    pub cross_folder: bool,
}

/// Captures full selection before dispatching blocking admission work.
pub struct PreparedPreview {
    request: PreviewRequest,
    cross_folder: bool,
}

impl ResourceService {
    pub fn prepare_preview(&self, input: PreviewInput) -> Result<PreparedPreview, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let selection =
            (input.level == RenderLevel::Full).then(|| self.queue.select_full(&input.path));
        Ok(PreparedPreview {
            request: PreviewRequest {
                selection,
                request_id: input.request_id,
                path: input.path,
                preview_dir: self.cache.preview_dir(),
                level: input.level,
                priority: input.priority,
                rank: input.rank,
            },
            cross_folder: input.cross_folder,
        })
    }

    /// Run on a blocking executor; decode remains in the owned render queue.
    pub fn resolve_preview(&self, prepared: PreparedPreview) -> Result<ImageProjection, String> {
        let (loading, receiver) = if prepared.cross_folder {
            self.queue.request_history(&self.runtime, prepared.request)
        } else {
            self.queue.request(&self.runtime, prepared.request)
        }?;
        let _ = self
            .runtime
            .publish(CoreEvent::ImageProjectionUpdated(Box::new(loading)));
        let projection = receiver
            .recv()
            .map_err(|e| format!("preview queue stopped: {e}"))??;
        if projection.result.is_none() {
            return Err("ready image projection has no artifact".into());
        }
        Ok(projection)
    }

    pub fn clear_preview_cache(&self) -> Result<CacheSettings, String> {
        let settings = self.cache.clear()?;
        self.queue.invalidate_all();
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppPaths, OxyApp};
    use std::time::Duration;

    #[test]
    fn prepared_full_preview_keeps_selection_order_before_blocking_dispatch() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.jpg");
        let second = directory.path().join("second.jpg");
        std::fs::write(&first, b"first source").unwrap();
        std::fs::write(&second, b"second source").unwrap();
        let (app, _) = OxyApp::open(AppPaths {
            data_dir: directory.path().join("data"),
            preview_dir: directory.path().join("previews"),
        })
        .unwrap();
        let input = |path, id: &str| PreviewInput {
            request_id: id.into(),
            path,
            level: RenderLevel::Full,
            priority: PreviewPriority::Loupe,
            rank: 0,
            cross_folder: false,
        };
        let prepared = app
            .resource_service
            .prepare_preview(input(first, "first"))
            .unwrap();
        let _newer = app
            .resource_service
            .prepare_preview(input(second.clone(), "second"))
            .unwrap();
        assert!(
            app.resource_service
                .resolve_preview(prepared)
                .unwrap_err()
                .contains("selection")
        );
        assert!(app.shutdown(Duration::from_secs(3)));
        assert!(
            app.resource_service
                .prepare_preview(input(second, "after-stop"))
                .is_err()
        );
    }
}
