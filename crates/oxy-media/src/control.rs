//! Cooperative controls shared by media work and its callers.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Maximum concurrent image workers, based on the CPUs available to this process.
pub fn image_worker_count() -> usize {
    static COUNT: std::sync::LazyLock<usize> =
        std::sync::LazyLock::new(|| std::thread::available_parallelism().map_or(1, usize::from));
    *COUNT
}

/// Full rendering has independent capacity, including a slot for a replacement
/// selection while a cancelled native call unwinds.
pub const fn loupe_worker_count() -> usize {
    2
}

/// Cloneable cooperative cancellation shared by queues and blocking workers.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Share cancellation with native adapters that poll an atomic flag.
    pub fn shared_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }
}
