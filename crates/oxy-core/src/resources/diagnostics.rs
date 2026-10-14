//! Nonblocking queue observations with retained samples for busy queues.
use super::{
    DebugSnapshotCache, LibraryIndexQueue, directory_tree::DirectoryTreeQueue,
    metadata::MetadataQueue, preview::PreviewQueue, types::DebugQueueSnapshot,
};
use std::{
    sync::Arc,
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
