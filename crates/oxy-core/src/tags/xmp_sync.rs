//! One coalescing, application-owned worker for the durable tag mirror queue.
use super::Tags;
use crate::{
    events::CoreEvent, files::FsCatalog, metadata::MetadataFacade, metadata::types::MetadataPatch,
    runtime::CoreRuntime, tags::types::TagSyncStatus,
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
    /// Only a changed snapshot becomes an event; a repeated drain stays quiet.
    reported: Mutex<Option<TagSyncStatus>>,
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
            reported: Mutex::new(None),
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
            // Announce the queued work before the drain and the emptied queue
            // after it, so the shell can show and clear its pending row without
            // polling for the state.
            self.publish_status();
            if let Err(error) = self.drain() {
                eprintln!("failed to run tag XMP sync: {error}");
            }
            self.publish_status();
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

    fn publish_status(&self) {
        let Ok(status) = self.tags.tag_sync_status() else {
            return;
        };
        let mut reported = self
            .reported
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if reported.as_ref() == Some(&status) {
            return;
        }
        *reported = Some(status.clone());
        drop(reported);
        let _ = self
            .runtime
            .publish(CoreEvent::TagSyncStatusChanged(status));
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
    fn status_events_announce_the_queue_then_its_drain() {
        use super::*;
        use std::time::Duration;

        let store = Arc::new(crate::storage::Store::in_memory().unwrap());
        let (runtime, events) = CoreRuntime::channel();
        let tags = Arc::new(Tags::new(store));
        let sync = Arc::new(TagXmpSync::new(
            runtime,
            tags.clone(),
            Arc::new(FsCatalog::default()),
            MetadataFacade::default(),
        ));
        let tag = tags.create_custom_tag(None, "Nature").unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.HIF");
        std::fs::write(&path, b"camera image bytes").unwrap();
        tags.set_asset_tag(std::slice::from_ref(&path), tag.id, true)
            .unwrap();
        sync.schedule();

        let mut queued = false;
        let mut drained = false;
        while !drained {
            let Ok(event) = events.recv_timeout(Duration::from_secs(3)) else {
                panic!("the mirror queue never reported an empty snapshot");
            };
            if let CoreEvent::TagSyncStatusChanged(status) = event {
                if status.pending_count == 1 && status.failed_count == 0 {
                    queued = true;
                }
                drained = queued && status.pending_count == 0 && status.failed_count == 0;
            }
        }
    }

    #[test]
    fn removal_updates_database_and_sidecar_without_restoring_the_tag() {
        use super::*;
        use crate::metadata::types::MetadataPatch;
        use oxy_media::types::AssetKind;

        for manual in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("photo.HIF");
            std::fs::write(&path, b"camera image bytes").unwrap();
            let tags = Tags::new(Arc::new(crate::storage::Store::in_memory().unwrap()));
            let files = FsCatalog::default();
            let metadata = MetadataFacade::default();
            metadata
                .patch_metadata(
                    &path,
                    AssetKind::Heif,
                    &MetadataPatch {
                        keywords: Some(vec!["Family".into(), "Other".into()]),
                        hierarchical_keywords: Some(vec!["People|Family".into(), "Other".into()]),
                        rating: Some(Some(4)),
                        ..MetadataPatch::default()
                    },
                )
                .unwrap();
            let current = metadata.read_metadata(&path, AssetKind::Heif).unwrap();
            tags.import_sidecar_tags(&path, &current.keywords, &current.hierarchical_keywords)
                .unwrap();
            let tag = tags
                .custom_tags()
                .unwrap()
                .into_iter()
                .find(|tag| tag.path == "People|Family")
                .unwrap();
            if manual {
                tags.set_asset_tag(std::slice::from_ref(&path), tag.id, true)
                    .unwrap();
                sync_path(&tags, &files, &metadata, &path).unwrap();
            }
            tags.set_asset_tag(std::slice::from_ref(&path), tag.id, false)
                .unwrap();
            sync_path(&tags, &files, &metadata, &path).unwrap();
            let current = metadata.read_metadata(&path, AssetKind::Heif).unwrap();
            assert_eq!(current.keywords, ["Other"]);
            assert_eq!(current.hierarchical_keywords, ["Other"]);
            assert_eq!(current.rating, Some(4));
            tags.import_sidecar_tags(&path, &current.keywords, &current.hierarchical_keywords)
                .unwrap();
            assert!(
                tags.asset_tag_source_kinds(&path, tag.id)
                    .unwrap()
                    .is_empty()
            );
            assert!(tags.pending_tag_sync_paths().unwrap().is_empty());
            assert_eq!(std::fs::read(&path).unwrap(), b"camera image bytes");
        }
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
