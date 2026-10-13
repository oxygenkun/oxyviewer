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
