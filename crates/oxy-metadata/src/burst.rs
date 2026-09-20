//! Incremental burst (continuous shooting) scanning.
//!
//! Grouping needs the burst values of every asset in a listing, but reading
//! them is exactly the work the interactive open path must avoid. The scanner
//! therefore works incrementally: each call parses only the paths it has not
//! seen, groups the caller's current listing, and keeps the values so the next
//! page costs only its own files. Grouping itself is a pure function in
//! `oxy_domain`, so a run that straddles two pages still collapses once both
//! are loaded.

use oxy_domain::{BurstFrame, BurstGroup};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Cached burst values keyed by path.
///
/// The values are immutable shooting data: editing a rating or a color label
/// never changes them, so entries are never invalidated.
#[derive(Default)]
pub struct BurstScanner {
    /// Serializes admission so overlapping incremental scans do not parse the
    /// same loaded prefix repeatedly while an earlier page is still running.
    scan_gate: Mutex<()>,
    frames: Mutex<HashMap<PathBuf, BurstFrame>>,
}

impl BurstScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse any unseen path, then group `paths` in the order given.
    ///
    /// `paths` is the caller's current listing, not a delta: passing the whole
    /// loaded page range every time is what lets a burst spanning two pages
    /// group correctly. Files that fail to parse take no part in grouping.
    pub fn scan(&self, paths: &[PathBuf]) -> Vec<BurstGroup> {
        // Pagination can publish another page before the previous scan
        // completes. Serialize the short-lived scanner as an admission gate;
        // the frame-cache mutex below still never spans file I/O.
        let _admission = self
            .scan_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let missing: Vec<PathBuf> = {
            let frames = self
                .frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            paths
                .iter()
                .filter(|path| !frames.contains_key(*path))
                .cloned()
                .collect()
        };

        // Parse outside the lock: reading a RAW is far slower than the copy.
        let mut parsed: HashMap<PathBuf, BurstFrame> = HashMap::with_capacity(missing.len());
        for path in missing {
            // Burst grouping needs three capture fields. Avoid the full
            // MetadataDocument path, which also builds raw-tag/focus results
            // and opens HEIF through libheif for XMP and chroma inspection.
            let capture = oxy_metadata_parser::tags(&path)
                .map(|tags| crate::capture::from_tags(&tags, &path))
                .unwrap_or_default();
            parsed.insert(
                path.clone(),
                BurstFrame {
                    path,
                    sequence_number: capture.sequence_number,
                    release_mode: capture.release_mode,
                    captured_at: capture.captured_at,
                },
            );
        }

        let frames = {
            let mut cache = self
                .frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.extend(parsed);
            paths
                .iter()
                .filter_map(|path| cache.get(path).cloned())
                .collect::<Vec<_>>()
        };
        oxy_domain::group_bursts(&frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Option<PathBuf> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name);
        path.is_file().then_some(path)
    }

    #[test]
    fn unreadable_paths_take_no_part_in_grouping() {
        let scanner = BurstScanner::new();
        let paths = vec![
            PathBuf::from("/does/not/exist/one.ARW"),
            PathBuf::from("/does/not/exist/two.ARW"),
        ];
        assert!(scanner.scan(&paths).is_empty());
        // The second call re-groups from the cache instead of re-parsing.
        assert!(scanner.scan(&paths).is_empty());
        let cached = scanner.frames.lock().expect("scanner poisoned").len();
        assert_eq!(cached, 2);
    }

    #[test]
    fn reads_burst_values_out_of_a_sony_hif() {
        // End-to-end: maker note -> capture normalization -> scanner cache.
        let Some(path) = fixture("DSC00511.HIF") else {
            eprintln!("skipping: tests/fixtures/DSC00511.HIF is absent");
            return;
        };
        let scanner = BurstScanner::new();
        assert!(scanner.scan(std::slice::from_ref(&path)).is_empty());
        let frame = scanner
            .frames
            .lock()
            .expect("scanner poisoned")
            .get(&path)
            .cloned();
        let frame = frame.expect("frame cached");
        assert_eq!(frame.sequence_number, Some(4));
        assert_eq!(frame.release_mode.as_deref(), Some("Continuous"));
        assert!(frame.captured_at.is_some());
    }
}
