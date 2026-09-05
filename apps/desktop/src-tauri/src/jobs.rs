pub(crate) mod directory_tree;
pub(crate) mod metadata;
pub(crate) mod preview;

use oxy_domain::{DebugQueueItem, DebugQueueState, LibraryIndexUpdate};
use oxy_library::{IndexProgress, IndexStage, Library};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::Emitter;

const LIBRARY_INDEX_UPDATED_EVENT: &str = "library-index-updated";
const LIBRARY_DIRECTORY_INDEX_UPDATED_EVENT: &str = "library-directory-index-updated";

#[derive(Clone)]
pub(crate) struct LibraryIndexQueue {
    library: Arc<Library>,
    work: Arc<Mutex<LibraryIndexWork>>,
}

#[derive(Default)]
struct LibraryIndexWork {
    pending: Vec<PathBuf>,
    active: Option<IndexProgress>,
}

impl LibraryIndexQueue {
    pub(crate) fn new(library: Arc<Library>) -> Self {
        Self {
            library,
            work: Arc::new(Mutex::new(LibraryIndexWork::default())),
        }
    }

    pub(crate) fn schedule(&self, app: tauri::AppHandle, root: PathBuf) {
        let root = root.canonicalize().unwrap_or(root);
        if !self.library.root_needs_index(&root).unwrap_or(false) {
            return;
        }
        {
            let mut work = self.work.lock().expect("library index queue poisoned");
            if work.pending.contains(&root)
                || work
                    .active
                    .as_ref()
                    .is_some_and(|progress| progress.root_path == root)
            {
                return;
            }
            work.pending.push(root.clone());
        }

        let queue = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let directory_event_app = app.clone();
            let mut directory_event_sent = false;
            let result = queue.library.index_root_with_progress(&root, |progress| {
                if progress.stage == IndexStage::Assets && !directory_event_sent {
                    directory_event_sent = true;
                    let _ = directory_event_app.emit(
                        LIBRARY_DIRECTORY_INDEX_UPDATED_EVENT,
                        LibraryIndexUpdate {
                            root_path: progress.root_path.clone(),
                            asset_count: progress.asset_count,
                            directory_count: progress.directory_count,
                        },
                    );
                }
                queue.update_progress(progress);
            });
            queue.finish(&root);
            match result {
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
                Err(error) => {
                    eprintln!("library indexing failed for {}: {error}", root.display());
                }
            }
        });
    }

    pub(crate) fn debug_snapshot(&self) -> DebugQueueState {
        let work = self.work.lock().expect("library index queue poisoned");
        DebugQueueState {
            name: "libraryIndex".into(),
            concurrency: 1,
            pending: work
                .pending
                .iter()
                .map(|root| library_index_debug_item(root, None))
                .collect(),
            active: work
                .active
                .as_ref()
                .map(|progress| {
                    vec![library_index_debug_item(
                        &progress.root_path,
                        Some(progress),
                    )]
                })
                .unwrap_or_default(),
        }
    }

    fn update_progress(&self, progress: IndexProgress) {
        let mut work = self.work.lock().expect("library index queue poisoned");
        work.pending.retain(|root| root != &progress.root_path);
        work.active = Some(progress);
    }

    fn finish(&self, root: &Path) {
        let mut work = self.work.lock().expect("library index queue poisoned");
        work.pending.retain(|pending| pending != root);
        if work
            .active
            .as_ref()
            .is_some_and(|progress| progress.root_path == root)
        {
            work.active = None;
        }
    }
}

fn library_index_debug_item(root: &Path, progress: Option<&IndexProgress>) -> DebugQueueItem {
    DebugQueueItem {
        key: root.to_string_lossy().into_owned(),
        path: Some(progress.map_or_else(
            || root.to_owned(),
            |progress| progress.current_directory.clone(),
        )),
        root_path: Some(root.to_owned()),
        stage: progress.map_or_else(
            || "waiting".into(),
            |progress| match progress.stage {
                IndexStage::Directories => "discoveringDirectories".into(),
                IndexStage::Assets => "indexingAssets".into(),
            },
        ),
        priority: "background".into(),
        rank: None,
        consumers: 1,
        pending_count: progress.map(|progress| progress.pending_directory_count),
        asset_count: progress.map(|progress| progress.asset_count),
        directory_count: progress.map(|progress| progress.directory_count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn library_index_snapshot_exposes_pending_and_active_progress() {
        let state = tempdir().unwrap();
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let library = Arc::new(Library::open(&state.path().join("library.sqlite")).unwrap());
        let queue = LibraryIndexQueue::new(library);
        queue.work.lock().unwrap().pending.push(root.clone());

        let pending = queue.debug_snapshot();
        assert_eq!(pending.name, "libraryIndex");
        assert_eq!(pending.pending[0].root_path.as_ref(), Some(&root));
        assert_eq!(pending.pending[0].stage, "waiting");

        let current_directory = root.join("current");
        queue.update_progress(IndexProgress {
            root_path: root.clone(),
            current_directory: current_directory.clone(),
            pending_directory_count: 12,
            asset_count: 345,
            directory_count: 67,
            stage: IndexStage::Directories,
        });

        let active = queue.debug_snapshot();
        assert!(active.pending.is_empty());
        assert_eq!(active.active[0].path.as_ref(), Some(&current_directory));
        assert_eq!(active.active[0].root_path.as_ref(), Some(&root));
        assert_eq!(active.active[0].pending_count, Some(12));
        assert_eq!(active.active[0].asset_count, Some(345));
        assert_eq!(active.active[0].directory_count, Some(67));
        assert_eq!(active.active[0].stage, "discoveringDirectories");
    }
}
