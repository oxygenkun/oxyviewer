pub(crate) mod directory_tree;
pub(crate) mod metadata;
pub(crate) mod preview;

use oxy_domain::LibraryIndexUpdate;
use oxy_library::Library;
use std::{path::PathBuf, sync::Arc};
use tauri::Emitter;

const LIBRARY_INDEX_UPDATED_EVENT: &str = "library-index-updated";

pub(crate) fn schedule_library_index(app: tauri::AppHandle, library: Arc<Library>, root: PathBuf) {
    tauri::async_runtime::spawn_blocking(move || match library.index_root(&root) {
        Ok(Some(stats)) => {
            let _ = app.emit(
                LIBRARY_INDEX_UPDATED_EVENT,
                LibraryIndexUpdate {
                    root_path: root,
                    asset_count: stats.asset_count,
                    directory_count: stats.directory_count,
                },
            );
        }
        Ok(None) => {}
        Err(error) => eprintln!("library indexing failed for {}: {error}", root.display()),
    });
}
