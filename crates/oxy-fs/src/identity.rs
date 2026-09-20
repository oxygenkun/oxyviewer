use sha2::{Digest, Sha256};
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

/// Observe the same canonical source identity used by media and analyzers.
pub fn observe_source_revision(path: &Path) -> io::Result<oxy_domain::SourceRevision> {
    let observed = observe_file(path)?;
    let mut hasher = Sha256::new();
    hasher.update(b"oxy-media-source-revision-v2\0");
    update_path(&mut hasher, &observed.canonical_path);
    hasher.update(observed.file_identity.as_bytes());
    hasher.update([0]);
    hasher.update(observed.size_bytes.to_le_bytes());
    hasher.update(observed.modified.as_bytes());
    let revision_id = format!("{:x}", hasher.finalize());
    Ok(oxy_domain::SourceRevision {
        canonical_path: observed.canonical_path,
        file_identity: observed.file_identity,
        size_bytes: observed.size_bytes,
        modified: observed.modified,
        revision_id,
    })
}

#[cfg(unix)]
fn update_path(hasher: &mut Sha256, path: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt;
    hasher.update(path.as_os_str().as_bytes());
    hasher.update([0]);
}

#[cfg(windows)]
fn update_path(hasher: &mut Sha256, path: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt;
    for code_unit in path.as_os_str().encode_wide() {
        hasher.update(code_unit.to_le_bytes());
    }
    hasher.update([0, 0]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_with_preserved_length_and_mtime_changes_revision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.jpg");
        fs::write(&path, b"original").unwrap();
        let old = observe_source_revision(&path).unwrap();
        let times =
            fs::FileTimes::new().set_modified(fs::metadata(&path).unwrap().modified().unwrap());
        let replacement = directory.path().join("replacement.jpg");
        fs::write(&replacement, b"replaced").unwrap();
        fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(times)
            .unwrap();
        fs::rename(&replacement, &path).unwrap();
        let new = observe_source_revision(&path).unwrap();
        assert_eq!(old.modified, new.modified);
        assert_eq!(old.size_bytes, new.size_bytes);
        assert_ne!(old.revision_id, new.revision_id);
    }

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
