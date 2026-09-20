pub(crate) mod directory_tree;
pub(crate) mod metadata;
pub(crate) mod preview;

use oxy_domain::{DebugQueueItem, DebugQueueState, LibraryIndexUpdate};
use oxy_fs::FsCatalog;
use oxy_library::{IndexProgress, IndexStage, Library};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::Emitter;

const LIBRARY_INDEX_UPDATED_EVENT: &str = "library-index-updated";
const LIBRARY_DIRECTORY_INDEX_UPDATED_EVENT: &str = "library-directory-index-updated";

/// Retain the last successful sample when a worker owns a queue lock.
/// This lock only copies diagnostics; no filesystem, database or queue work
/// runs while it is held.
#[derive(Default)]
pub(crate) struct DebugSnapshotCache {
    queues: Mutex<HashMap<String, DebugQueueState>>,
}

impl DebugSnapshotCache {
    pub(crate) fn remember(
        &self,
        observations: [(&str, Option<DebugQueueState>); 5],
    ) -> (Vec<DebugQueueState>, Vec<String>) {
        let mut previous = self.queues.lock().expect("snapshot cache poisoned");
        let mut queues = Vec::new();
        let mut stale = Vec::new();
        for (name, observed) in observations {
            if let Some(observed) = observed {
                previous.insert(name.to_owned(), observed);
            } else {
                stale.push(name.to_owned());
            }
            if let Some(queue) = previous.get(name) {
                queues.push(queue.clone());
            }
        }
        (queues, stale)
    }
}

#[derive(Clone)]
pub(crate) struct LibraryIndexQueue {
    library: Arc<Library>,
    files: Arc<FsCatalog>,
    work: Arc<Mutex<LibraryIndexWork>>,
}

#[derive(Default)]
struct LibraryIndexWork {
    pending: Vec<PathBuf>,
    active: Option<IndexProgress>,
}

impl LibraryIndexQueue {
    pub(crate) fn new(library: Arc<Library>, files: Arc<FsCatalog>) -> Self {
        Self {
            library,
            files,
            work: Arc::new(Mutex::new(LibraryIndexWork::default())),
        }
    }

    pub(crate) fn schedule(&self, app: tauri::AppHandle, root: PathBuf) {
        // Callers supply registered canonical paths. Do not stat a NAS on the
        // command/UI thread just to enqueue background work.
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
            queue.library.foreground.wait_for_background();
            let directory_event_app = app.clone();
            let mut directory_event_sent = false;
            let result = queue.library.index_root_with_progress_and_directory_scan(
                &root,
                |progress| {
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
                },
                |directory| {
                    queue.files.scan_index_directories_cached(directory, || {
                        queue.library.foreground.wait_for_background();
                    })
                },
            );
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

    pub(crate) fn debug_snapshot(&self) -> Option<DebugQueueState> {
        let work = self.work.try_lock().ok()?;
        Some(DebugQueueState {
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
        })
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

    #[test]
    fn busy_snapshot_keeps_the_last_sample_and_marks_it_stale() {
        let cache = DebugSnapshotCache::default();
        let sample = DebugQueueState {
            name: "thumbnail".into(),
            concurrency: 3,
            pending: vec![],
            active: vec![],
        };
        let observations = |value| {
            [
                ("loupe", None),
                ("thumbnail", value),
                ("metadata", None),
                ("directoryTree", None),
                ("libraryIndex", None),
            ]
        };
        let (first, stale) = cache.remember(observations(Some(sample.clone())));
        assert_eq!(first, vec![sample.clone()]);
        assert!(!stale.iter().any(|name| name == "thumbnail"));
        let (busy, stale) = cache.remember(observations(None));
        assert_eq!(busy, vec![sample]);
        assert!(stale.iter().any(|name| name == "thumbnail"));
    }
    use tempfile::tempdir;

    #[test]
    fn library_index_snapshot_exposes_pending_and_active_progress() {
        let state = tempdir().unwrap();
        let root = tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let library = Arc::new(Library::open(&state.path().join("library.sqlite")).unwrap());
        let queue = LibraryIndexQueue::new(library, Arc::new(FsCatalog::default()));
        queue.work.lock().unwrap().pending.push(root.clone());

        let pending = queue.debug_snapshot().unwrap();
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

        let active = queue.debug_snapshot().unwrap();
        assert!(active.pending.is_empty());
        assert_eq!(active.active[0].path.as_ref(), Some(&current_directory));
        assert_eq!(active.active[0].root_path.as_ref(), Some(&root));
        assert_eq!(active.active[0].pending_count, Some(12));
        assert_eq!(active.active[0].asset_count, Some(345));
        assert_eq!(active.active[0].directory_count, Some(67));
        assert_eq!(active.active[0].stage, "discoveringDirectories");
    }
}
