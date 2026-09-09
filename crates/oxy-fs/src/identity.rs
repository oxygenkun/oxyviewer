use std::{fs, io, path::Path, path::PathBuf};

/// A point-in-time observation of one filesystem object.
///
/// The identity and metadata are read from the same open file handle. The
/// identity is platform-specific and opaque to callers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileObservation {
    pub canonical_path: PathBuf,
    pub file_identity: String,
    pub size_bytes: u64,
    pub modified: String,
}

pub fn observe_file(path: &Path) -> io::Result<FileObservation> {
    let canonical_path = path.canonicalize()?;
    let file = fs::File::open(&canonical_path)?;
    let metadata = file.metadata()?;
    let file_identity = platform_file_identity(&file, &metadata)?;
    let modified = platform_modified(&metadata);

    Ok(FileObservation {
        canonical_path,
        file_identity,
        size_bytes: metadata.len(),
        modified,
    })
}

#[cfg(unix)]
fn platform_file_identity(_file: &fs::File, metadata: &fs::Metadata) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn platform_file_identity(file: &fs::File, _metadata: &fs::Metadata) -> io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle},
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `file` owns a valid handle for the duration of this call, and
    // `information` points to writable storage of the required type.
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information)
            .map_err(io::Error::other)?;
    }
    let file_index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok(format!(
        "{}:{}",
        information.dwVolumeSerialNumber, file_index
    ))
}

#[cfg(unix)]
fn platform_modified(metadata: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    format!("{}:{}", metadata.mtime(), metadata.mtime_nsec())
}

#[cfg(windows)]
fn platform_modified(metadata: &fs::Metadata) -> String {
    use std::os::windows::fs::MetadataExt;
    metadata.last_write_time().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_links_share_file_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.jpg");
        let link = directory.path().join("linked.jpg");
        fs::write(&path, b"source").unwrap();
        fs::hard_link(&path, &link).unwrap();

        let source = observe_file(&path).unwrap();
        let linked = observe_file(&link).unwrap();

        assert_ne!(source.canonical_path, linked.canonical_path);
        assert_eq!(source.file_identity, linked.file_identity);
        assert_eq!(source.size_bytes, 6);
        assert_eq!(source.modified, linked.modified);
    }
}
