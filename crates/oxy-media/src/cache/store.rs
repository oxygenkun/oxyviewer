use crate::MediaError;
use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheUsage {
    pub size_bytes: u64,
    pub file_count: usize,
}

#[derive(Debug)]
struct CacheEntry {
    path: PathBuf,
    size_bytes: u64,
    modified: SystemTime,
}

/// Measures only regular files directly inside the app-owned preview folder.
/// Preview artifacts are intentionally flat, so unrelated nested content is
/// never traversed or counted.
pub fn preview_cache_usage(cache_dir: &Path) -> Result<CacheUsage, MediaError> {
    let entries = preview_cache_entries(cache_dir)?;
    Ok(CacheUsage {
        size_bytes: entries.iter().map(|entry| entry.size_bytes).sum(),
        file_count: entries.len(),
    })
}

/// Removes least-recently-modified preview artifacts until the configured
/// budget is met. The artifact returned by the current request can be
/// protected so pruning never races the webview's first read of that file.
pub fn prune_preview_cache(
    cache_dir: &Path,
    max_size_bytes: u64,
    protected_path: Option<&Path>,
) -> Result<CacheUsage, MediaError> {
    let mut entries = preview_cache_entries(cache_dir)?;
    entries.sort_by_key(|entry| entry.modified);
    let mut total = entries.iter().map(|entry| entry.size_bytes).sum::<u64>();
    let mut file_count = entries.len();
    for entry in entries {
        if total <= max_size_bytes {
            break;
        }
        if protected_path.is_some_and(|protected| protected == entry.path) {
            continue;
        }
        match fs::remove_file(&entry.path) {
            Ok(()) => {
                total = total.saturating_sub(entry.size_bytes);
                file_count = file_count.saturating_sub(1);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(CacheUsage {
        size_bytes: total,
        file_count,
    })
}

/// Clears regular preview artifacts without recursively deleting the selected
/// directory. This remains safe even if a future configuration is malformed.
pub fn clear_preview_cache(cache_dir: &Path) -> Result<CacheUsage, MediaError> {
    for entry in preview_cache_entries(cache_dir)? {
        match fs::remove_file(entry.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    preview_cache_usage(cache_dir)
}

fn preview_cache_entries(cache_dir: &Path) -> Result<Vec<CacheEntry>, MediaError> {
    // Check the operation itself, not exists(): the directory can disappear
    // between those calls, and exists() also masks permission errors.
    let directory = match fs::read_dir(cache_dir) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    collect_cache_entries(directory)
}

fn collect_cache_entries(
    directory: impl IntoIterator<Item = std::io::Result<fs::DirEntry>>,
) -> Result<Vec<CacheEntry>, MediaError> {
    let mut entries = Vec::new();
    for entry in directory {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        // Atomic cache commits rename temporary files in this same directory.
        // A listed entry may already be gone by the time we inspect it.
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() {
            continue;
        }
        entries.push(CacheEntry {
            path: entry.path(),
            size_bytes: metadata.len(),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unix DirEntry reads metadata from the path; Windows may retain it from
    // enumeration, so a vanished entry there can legitimately remain in the
    // approximate snapshot until the next scan.
    #[cfg(unix)]
    #[test]
    fn scan_tolerates_atomic_commit_between_listing_and_metadata() {
        let cache = tempfile::tempdir().unwrap();
        let temporary = tempfile::NamedTempFile::new_in(cache.path()).unwrap();
        fs::write(temporary.path(), b"preview").unwrap();
        let retained = cache.path().join("retained.jpg");
        fs::write(&retained, b"keep").unwrap();
        let listed = fs::read_dir(cache.path()).unwrap().collect::<Vec<_>>();

        // Deterministically reproduce the writer winning the race after the
        // pruner enumerates its temporary file, before it reads metadata.
        let destination = cache.path().join("committed.jpg");
        temporary.persist_noclobber(&destination).unwrap();
        let entries = collect_cache_entries(listed).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, retained);
        assert_eq!(entries[0].size_bytes, 4);
        assert_eq!(fs::read(&destination).unwrap(), b"preview");
        assert_eq!(preview_cache_usage(cache.path()).unwrap().file_count, 2);
    }

    #[test]
    fn scan_skips_missing_iterator_entries_but_preserves_other_errors() {
        let cache = tempfile::tempdir().unwrap();
        let retained = cache.path().join("retained.jpg");
        fs::write(&retained, b"keep").unwrap();
        let entry = fs::read_dir(cache.path()).unwrap().next().unwrap();
        let entries = collect_cache_entries([
            Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
            entry,
        ])
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, retained);

        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::Other,
        ] {
            assert!(matches!(
                collect_cache_entries([Err(std::io::Error::from(kind))]),
                Err(MediaError::Io(error)) if error.kind() == kind
            ));
        }
    }

    #[test]
    fn missing_directory_is_handled_at_enumeration_boundary() {
        let cache = tempfile::tempdir().unwrap();
        let removed = cache.path().join("removed");
        fs::create_dir(&removed).unwrap();
        fs::remove_dir(&removed).unwrap();
        assert!(preview_cache_entries(&removed).unwrap().is_empty());

        let not_a_directory = cache.path().join("file");
        fs::write(&not_a_directory, b"keep").unwrap();
        assert!(preview_cache_usage(&not_a_directory).is_err());
        assert!(prune_preview_cache(&not_a_directory, 0, None).is_err());
        assert!(clear_preview_cache(&not_a_directory).is_err());
        assert_eq!(fs::read(&not_a_directory).unwrap(), b"keep");
    }

    #[test]
    fn missing_cache_is_empty_and_is_not_created() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("missing");
        for usage in [
            preview_cache_usage(&cache).unwrap(),
            prune_preview_cache(&cache, 0, None).unwrap(),
            clear_preview_cache(&cache).unwrap(),
        ] {
            assert_eq!(usage.size_bytes, 0);
            assert_eq!(usage.file_count, 0);
        }
        assert!(!cache.exists());
    }

    #[test]
    fn usage_counts_only_direct_regular_files() {
        let cache = tempfile::tempdir().unwrap();
        fs::write(cache.path().join("preview.jpg"), [1_u8; 4]).unwrap();
        let nested = cache.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("ignored.jpg"), [2_u8; 8]).unwrap();

        let usage = preview_cache_usage(cache.path()).unwrap();
        assert_eq!(usage.size_bytes, 4);
        assert_eq!(usage.file_count, 1);
    }

    #[test]
    fn pruning_keeps_protected_artifact_even_over_budget() {
        let cache = tempfile::tempdir().unwrap();
        let protected = cache.path().join("current.jpg");
        fs::write(&protected, [1_u8; 4]).unwrap();

        let usage = prune_preview_cache(cache.path(), 0, Some(&protected)).unwrap();
        assert_eq!(usage.size_bytes, 4);
        assert_eq!(usage.file_count, 1);
        assert!(protected.exists());
    }

    #[test]
    fn preview_cache_pruning_respects_budget_and_protected_artifact() {
        let cache = tempfile::tempdir().unwrap();
        let older = cache.path().join("older.jpg");
        let protected = cache.path().join("current.jpg");
        fs::write(&older, [1_u8; 4]).unwrap();
        fs::write(&protected, [2_u8; 4]).unwrap();

        let usage = prune_preview_cache(cache.path(), 4, Some(&protected)).unwrap();

        assert_eq!(usage.size_bytes, 4);
        assert_eq!(usage.file_count, 1);
        assert!(!older.exists());
        assert!(protected.exists());
    }

    #[test]
    fn clearing_preview_cache_does_not_traverse_nested_directories() {
        let cache = tempfile::tempdir().unwrap();
        fs::write(cache.path().join("preview.jpg"), [1_u8; 4]).unwrap();
        let nested = cache.path().join("unrelated");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("keep.txt"), b"keep").unwrap();

        let usage = clear_preview_cache(cache.path()).unwrap();

        assert_eq!(usage.size_bytes, 0);
        assert!(nested.join("keep.txt").exists());
    }
}
