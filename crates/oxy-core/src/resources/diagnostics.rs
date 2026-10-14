//! Nonblocking queue observations with retained samples for busy queues.
use super::{
    metadata::MetadataQueue,
    preview::PreviewQueue,
    types::{DebugQueueSnapshot, DebugQueueState},
};
use crate::{browse::tree_queue::DirectoryTreeQueue, library::scheduling::LibraryIndexQueue};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub struct QueueDiagnostics {
    pub(crate) preview: PreviewQueue,
    pub(crate) metadata: MetadataQueue,
    pub(crate) directory_tree: DirectoryTreeQueue,
    pub(crate) library_index: LibraryIndexQueue,
    pub(crate) snapshots: Arc<DebugSnapshotCache>,
}

impl QueueDiagnostics {
    /// Run on the host's blocking executor; every queue uses try_lock.
    pub fn collect(&self, requested_at: Instant) -> Result<DebugQueueSnapshot, String> {
        let collection_started = Instant::now();
        let worker_wait_micros = requested_at
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX);
        let captured_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        let [loupe, thumbnail] = self.preview.debug_snapshot();
        let (queues, stale_queues) = self.snapshots.remember([
            ("loupe", loupe),
            ("thumbnail", thumbnail),
            ("metadata", self.metadata.debug_snapshot()),
            ("directoryTree", self.directory_tree.debug_snapshot()),
            ("libraryIndex", self.library_index.debug_snapshot()),
        ]);
        Ok(DebugQueueSnapshot {
            captured_at_unix_ms,
            worker_wait_micros,
            collection_micros: collection_started
                .elapsed()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX),
            queues,
            stale_queues,
        })
    }
}

/// Retain the last successful sample when a worker owns a queue lock.
/// This lock only copies diagnostics; no filesystem, database or queue work
/// runs while it is held.
#[derive(Default)]
pub(crate) struct DebugSnapshotCache {
    queues: Mutex<HashMap<String, DebugQueueState>>,
}

impl DebugSnapshotCache {
    pub fn remember(
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
}
