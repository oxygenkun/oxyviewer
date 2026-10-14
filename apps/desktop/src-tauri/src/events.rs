use oxy_core::events::CoreEvent;
use std::sync::{Mutex, mpsc::Receiver};
use std::thread::JoinHandle;
use tauri::Manager;
use tauri::{AppHandle, Emitter};

pub(crate) struct EventAdapter(Mutex<Option<JoinHandle<()>>>);

impl EventAdapter {
    pub(crate) fn join(&self) {
        if let Some(thread) = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = thread.join();
        }
    }
}

pub(crate) fn forward(app: AppHandle, events: Receiver<CoreEvent>) {
    let host = app.clone();
    let worker = std::thread::Builder::new()
        .name("oxy-desktop-events".into())
        .spawn(move || {
            for event in events {
                match event {
                    CoreEvent::ImageProjectionUpdated(value) => {
                        let _ = app.emit("image-projection-updated", value);
                    }
                    CoreEvent::MetadataProjectionUpdated(value) => {
                        let _ = app.emit("metadata-projection-updated", value);
                    }
                    CoreEvent::AssetTagsUpdated(path) => {
                        let _ = app.emit("asset-tags-updated", path);
                    }
                    CoreEvent::HeifTilePublished(tile) => {
                        let _ = app.emit("heif-tile-ready", heif_tile_event(*tile));
                    }
                    CoreEvent::HeifStatusUpdated(value) => {
                        let _ = app.emit("heif-decode-status", value);
                    }
                    CoreEvent::DirectoryTreeUpdated(value) => {
                        let _ = app.emit("directory-tree-updated", value);
                    }
                    CoreEvent::DirectoryBrowseProgress(value) => {
                        let _ = app.emit("directory-browse-progress", value);
                    }
                    CoreEvent::LibraryDirectoriesIndexed(value) => {
                        let _ = app.emit("library-directory-index-updated", value);
                    }
                    CoreEvent::LibraryIndexed(value) => {
                        let _ = app.emit("library-index-updated", value);
                    }
                    CoreEvent::Shutdown => break,
                }
            }
        })
        .expect("failed to start desktop event adapter");
    host.manage(EventAdapter(Mutex::new(Some(worker))));
}

fn heif_tile_event(tile: oxy_media::HeifTilePublication) -> oxy_core::contracts::HeifTileReady {
    oxy_core::contracts::HeifTileReady {
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
