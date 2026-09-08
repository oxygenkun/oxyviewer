//! Helpers shared by preview artifact pipelines.

use crate::{MediaError, cache::preview_cache_key, media_source::cached_preview_result};
use oxy_domain::{PreviewKind, PreviewResult, RenderLevel};
use std::{path::Path, time::Instant};

/// Cache sizes shared by every format's progressive pipeline. A request for a
/// smaller size may be satisfied by a larger cached entry.
pub(crate) const PREVIEW_CACHE_SIZES: [u32; 2] = [512, 4_096];

pub(crate) fn duration_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

/// Returns the closest larger decoded JPEG produced under the same cache
/// version. Representation-specific pipelines must not use this helper for
/// embedded camera JPEGs or developed RAW artifacts.
pub(crate) fn larger_cached_decoded_preview(
    path: &Path,
    cache_dir: &Path,
    cache_version: &str,
    max_size: u32,
    level: RenderLevel,
) -> Result<Option<PreviewResult>, MediaError> {
    for candidate_size in PREVIEW_CACHE_SIZES
        .iter()
        .copied()
        .filter(|&size| size > max_size)
    {
        let key = preview_cache_key(path, cache_version, candidate_size)?;
        let candidate = cache_dir.join(format!("{key}.decoded.jpg"));
        if let Some(result) = cached_preview_result(candidate, PreviewKind::Decoded, level)? {
            return Ok(Some(result));
        }
    }
    Ok(None)
}
