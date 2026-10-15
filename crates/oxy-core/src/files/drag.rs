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
}
