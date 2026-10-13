//! One coalescing, application-owned worker for the durable tag mirror queue.
use super::Tags;
use crate::{
    files::FsCatalog, metadata::MetadataFacade, metadata::types::MetadataPatch,
    runtime::CoreRuntime,
};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Work {
    running: bool,
    requested: bool,
}

pub struct TagXmpSync {
    runtime: CoreRuntime,
    tags: Arc<Tags>,
    files: Arc<FsCatalog>,
    metadata: MetadataFacade,
    work: Mutex<Work>,
}

impl TagXmpSync {
    pub(crate) fn new(
        runtime: CoreRuntime,
        tags: Arc<Tags>,
        files: Arc<FsCatalog>,
        metadata: MetadataFacade,
    ) -> Self {
        Self {
            runtime,
            tags,
            files,
            metadata,
            work: Mutex::new(Work::default()),
        }
    }

    pub fn schedule(self: &Arc<Self>) {
        let mut work = self
            .work
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.runtime.is_stopping() {
            return;
        }
        work.requested = true;
        if work.running {
            return;
        }
        work.running = true;
        drop(work);
        let sync = Arc::clone(self);
        if self
            .runtime
            .spawn_named("oxy-tag-xmp-sync".into(), move || sync.run())
            .is_err()
        {
            self.work
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .running = false;
        }
    }

    fn run(&self) {
        loop {
            self.work
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .requested = false;
            if let Err(error) = self.drain() {
                eprintln!("failed to run tag XMP sync: {error}");
            }
            let mut work = self
                .work
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.runtime.is_stopping() || !work.requested {
                work.running = false;
                return;
            }
        }
    }

    fn drain(&self) -> Result<(), String> {
        for path in self
            .tags
            .pending_tag_sync_paths()
            .map_err(|error| error.to_string())?
        {
            if self.runtime.is_stopping() {
                break;
            }
            if let Err(error) = sync_path(&self.tags, &self.files, &self.metadata, &path) {
                self.tags
                    .fail_tag_xmp_sync(&path, &error)
                    .map_err(|db_error| db_error.to_string())?;
            }
        }
        Ok(())
    }
}

fn sync_path(
    tags: &Tags,
    files: &FsCatalog,
    metadata: &MetadataFacade,
    path: &std::path::Path,
) -> Result<(), String> {
    let asset = files.get_asset(path).map_err(|error| error.to_string())?;
    let current = metadata
        .read_metadata(path, asset.kind)
        .map_err(|error| error.to_string())?;
    let payload = tags
        .tag_xmp_payload(path)
        .map_err(|error| error.to_string())?;
    let subjects = merge_managed_values(
        current.keywords,
        &payload.previous_subjects,
        &payload.subjects,
    );
    let hierarchical = merge_managed_values(
        current.hierarchical_keywords,
        &payload.previous_hierarchical,
        &payload.hierarchical,
    );
    metadata
        .patch_metadata(
            path,
            asset.kind,
            &MetadataPatch {
                keywords: Some(subjects),
                hierarchical_keywords: Some(hierarchical),
                ..MetadataPatch::default()
            },
        )
        .map_err(|error| error.to_string())?;
    tags.complete_tag_xmp_sync(path, &payload.subjects, &payload.hierarchical)
        .map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        files.invalidate_directory(parent);
    }
    Ok(())
}

fn merge_managed_values(
    current: Vec<String>,
    previous_managed: &[String],
    desired: &[String],
) -> Vec<String> {
    let previous = previous_managed.iter().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    current
        .into_iter()
        .filter(|value| !previous.contains(value))
        .chain(desired.iter().cloned())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::merge_managed_values;

    #[test]
    fn publication_bursts_share_one_owned_worker_and_stop_after_io_unblocks() {
        use super::*;
        let store = Arc::new(crate::storage::Store::in_memory().unwrap());
        let runtime = CoreRuntime::default();
        let sync = Arc::new(TagXmpSync::new(
            runtime.clone(),
            Arc::new(Tags::new(store.clone())),
            Arc::new(FsCatalog::default()),
            MetadataFacade::new(None),
        ));
        let writer = store.write();
        for _ in 0..100 {
            sync.schedule();
        }
        assert_eq!(runtime.active_workers(), 1);
        runtime.stop();
        drop(writer);
        assert!(runtime.shutdown(std::time::Duration::from_secs(2)));
        assert!(!sync.work.lock().unwrap().running);
    }

    #[test]
    fn replaces_only_values_managed_by_oxyviewer() {
        assert_eq!(
            merge_managed_values(
                vec!["external".into(), "old".into()],
                &["old".into()],
                &["new".into()]
            ),
            ["external", "new"]
        );
    }
}
