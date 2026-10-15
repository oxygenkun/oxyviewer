//! Drag feedback reuses the thumbnail queue, cache, geometry and resource leases.
use super::{PreviewInput, ResourceService};
use oxy_media::types::{PreviewPriority, RenderLevel};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DRAG: AtomicU64 = AtomicU64::new(0);

impl ResourceService {
    /// Run on a blocking executor. Only the leading photo is needed; selection
    /// size changes the card stack, never the amount of source decode work.
    pub fn drag_image(&self, path: PathBuf, count: usize) -> Result<Vec<u8>, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let thumbnail = (|| {
            let prepared = self.prepare_preview(PreviewInput {
                request_id: format!("native-drag:{}", NEXT_DRAG.fetch_add(1, Ordering::Relaxed)),
                path,
                level: RenderLevel::Thumbnail,
                priority: PreviewPriority::Visible,
                rank: 0,
                cross_folder: true,
            })?;
            let projection = self.resolve_preview(prepared)?;
            let result = projection.result.ok_or("drag thumbnail has no artifact")?;
            let descriptor = result.resource.ok_or("drag thumbnail has no resource")?;
            let registry = oxy_media::shared_resource_registry();
            let lease = registry
                .resolve(&descriptor.resource_id)
                .ok_or("drag thumbnail expired")?;
            let bytes = registry
                .materialize(&lease)
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((bytes, result.geometry))
        })();
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let (bytes, geometry) = match &thumbnail {
            Ok((bytes, geometry)) => (Some(bytes.as_slice()), *geometry),
            Err(_) => (None, None),
        };
        oxy_media::drag_image::photo_drag_image(bytes, geometry, count)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppPaths, OxyApp};
    use std::time::Duration;

    #[test]
    fn drag_feedback_uses_the_originals_thumbnail_for_both_single_and_stack() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.png");
        image::RgbaImage::from_pixel(320, 200, image::Rgba([230, 40, 90, 255]))
            .save(&path)
            .unwrap();
        let (app, _) = OxyApp::open(AppPaths {
            data_dir: directory.path().join("data"),
            preview_dir: directory.path().join("previews"),
        })
        .unwrap();
        let single = app.resource_service.drag_image(path.clone(), 1).unwrap();
        let stack = app.resource_service.drag_image(path.clone(), 10).unwrap();
        let single = image::load_from_memory(&single).unwrap().into_rgba8();
        let stack = image::load_from_memory(&stack).unwrap().into_rgba8();
        assert_eq!(single.dimensions(), (140, 92));
        assert_eq!(stack.dimensions(), (156, 108));
        assert_eq!(single.get_pixel(20, 20).0, [230, 40, 90, 255]);
        assert_eq!(stack.get_pixel(20, 20).0, [230, 40, 90, 255]);
        assert!(app.shutdown(Duration::from_secs(3)));
        assert!(app.resource_service.drag_image(path, 1).is_err());
    }
}
