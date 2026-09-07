use std::time::Instant;

/// Cache sizes shared by every format's progressive pipeline. A request for a
/// smaller size may be satisfied by a larger cached entry.
pub(crate) const PREVIEW_CACHE_SIZES: [u32; 2] = [512, 4_096];

pub(crate) fn duration_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}
