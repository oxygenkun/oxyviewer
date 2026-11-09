//! Read-only preparation of original files for an operating-system drag.
use std::{collections::HashSet, io, path::PathBuf};

/// Validate off the UI thread. Dragging offers copies; it performs no file or
/// database mutation, and never adds sidecars or generated previews.
pub fn prepare_drag_paths(paths: Vec<PathBuf>) -> io::Result<Vec<PathBuf>> {
    if paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "No files to drag",
        ));
    }
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(paths.len());
    for path in paths {
        if !path.is_absolute() || !path.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Not an absolute file path: {}", path.display()),
            ));
        }
        if seen.insert(path.clone()) {
            result.push(path);
        }
    }
    Ok(result)
}

/// Run on a blocking worker. Offer Shell-compatible spellings only when they
/// resolve to the same original, preserving verbatim-only names and volumes.
#[cfg(windows)]
pub fn shell_drag_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|path| super::external_apps::application_path(path).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_originals_in_order_without_sidecars_or_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("照片 one.HIF");
        let second = directory.path().join("two.NEF");
        let sidecar = directory.path().join("two.xmp");
        for path in [&first, &second, &sidecar] {
            std::fs::write(path, b"original").unwrap();
        }
        assert_eq!(
            prepare_drag_paths(vec![second.clone(), first.clone(), second.clone()]).unwrap(),
            vec![second.clone(), first.clone()]
        );
        for path in [first, second, sidecar] {
            assert_eq!(std::fs::read(path).unwrap(), b"original");
        }
    }

    #[test]
    fn rejects_empty_relative_missing_and_directory_paths() {
        let directory = tempfile::tempdir().unwrap();
        assert!(prepare_drag_paths(vec![]).is_err());
        assert!(prepare_drag_paths(vec![PathBuf::from("relative.jpg")]).is_err());
        assert!(prepare_drag_paths(vec![directory.path().join("missing.jpg")]).is_err());
        assert!(prepare_drag_paths(vec![directory.path().to_owned()]).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn shell_spelling_keeps_verbatim_only_originals_distinct() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let plain = root.join("照片 one.jpg");
        let special = root.join("照片 one.jpg.");
        std::fs::write(&plain, b"plain").unwrap();
        std::fs::write(&special, b"special").unwrap();
        let paths = shell_drag_paths(&[plain.clone(), special.clone()]);
        assert!(!paths[0].as_os_str().to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(std::fs::canonicalize(&paths[0]).unwrap(), plain);
        assert_eq!(paths[1], special);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"plain");
        assert_eq!(std::fs::read(&paths[1]).unwrap(), b"special");
    }
}
