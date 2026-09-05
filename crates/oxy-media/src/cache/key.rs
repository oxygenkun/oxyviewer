use crate::MediaError;
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::Path,
};

pub(crate) fn preview_cache_key(
    path: &Path,
    backend: &str,
    max_size: u32,
) -> Result<String, MediaError> {
    // Key previews by physical file identity rather than by the library root
    // used to reach it. Parent/child roots (and symlink aliases) therefore
    // converge on one cache entry for the same source image.
    let canonical_path = path.canonicalize()?;
    let metadata = fs::metadata(&canonical_path)?;
    let mut hasher = DefaultHasher::new();
    canonical_path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata.modified().ok().hash(&mut hasher);
    backend.hash(&mut hasher);
    max_size.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn cache_key_tracks_source_identity_size_and_modification_time() {
        use std::{
            fs::FileTimes,
            time::{Duration, SystemTime},
        };

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.jpg");
        let other = directory.path().join("other.jpg");
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for path in [&source, &other] {
            fs::write(path, b"one").unwrap();
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(FileTimes::new().set_modified(modified))
                .unwrap();
        }
        let initial = preview_cache_key(&source, "jpeg", 512).unwrap();
        assert_eq!(initial, preview_cache_key(&source, "jpeg", 512).unwrap());
        assert_ne!(initial, preview_cache_key(&other, "jpeg", 512).unwrap());

        // Isolate length from mtime: keep the timestamp fixed while resizing.
        fs::write(&source, b"longer").unwrap();
        let file = fs::File::options().write(true).open(&source).unwrap();
        file.set_times(FileTimes::new().set_modified(modified))
            .unwrap();
        let resized = preview_cache_key(&source, "jpeg", 512).unwrap();
        assert_ne!(initial, resized);

        file.set_times(FileTimes::new().set_modified(modified + Duration::from_secs(60)))
            .unwrap();
        assert_ne!(resized, preview_cache_key(&source, "jpeg", 512).unwrap());
    }

    #[test]
    fn missing_source_does_not_produce_a_key() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            preview_cache_key(&directory.path().join("missing.jpg"), "jpeg", 512),
            Err(MediaError::Io(_))
        ));
    }

    #[test]
    fn cache_key_changes_with_backend_and_size() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.jpg");
        RgbImage::from_pixel(4, 3, Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();

        let first = preview_cache_key(&path, "one", 512).unwrap();
        let different_backend = preview_cache_key(&path, "two", 512).unwrap();
        let different_size = preview_cache_key(&path, "one", 2_048).unwrap();

        assert_ne!(first, different_backend);
        assert_ne!(first, different_size);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn cache_key_is_shared_across_paths_to_the_same_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.jpg");
        let alias = directory.path().join("image-alias.jpg");
        RgbImage::from_pixel(4, 3, Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        std::os::unix::fs::symlink(&path, &alias).unwrap();

        assert_eq!(
            preview_cache_key(&path, "jpeg", 512).unwrap(),
            preview_cache_key(&alias, "jpeg", 512).unwrap()
        );
    }
}
