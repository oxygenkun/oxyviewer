//! Memory-mapped file I/O (F4).

use std::path::Path;

use crate::core::{Error, FileType, Result};

/// A memory-mapped file providing zero-copy access to its contents.
pub struct MappedFile {
    mmap: memmap2::Mmap,
}

impl MappedFile {
    /// Open and memory-map a file.
    pub fn open(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(Error::Io)?;
        // SAFETY: We treat the mapped region as read-only and do not hold references
        // across file modifications (the file is opened read-only).
        let mmap = unsafe { memmap2::Mmap::map(&file).map_err(Error::Io)? };
        Ok(Self { mmap })
    }

    /// Returns the file contents as a byte slice.
    pub fn as_bytes(&self) -> &[u8] {
        &self.mmap
    }

    /// Returns the file size in bytes.
    pub fn len(&self) -> usize {
        self.mmap.len()
    }

    /// Returns true if the file is empty.
    pub fn is_empty(&self) -> bool {
        self.mmap.is_empty()
    }

    /// Detect the file type from magic bytes.
    pub fn file_type(&self) -> Option<FileType> {
        FileType::detect(self.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn map_and_read() {
        const DATA: &[u8] = b"hello siftx";

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(DATA).unwrap();
        tmp.flush().unwrap();

        let mapped = MappedFile::open(tmp.path()).unwrap();
        assert_eq!(mapped.as_bytes(), DATA);
        assert_eq!(mapped.len(), DATA.len());
        assert!(!mapped.is_empty());
    }

    #[test]
    fn detect_type_from_mapped() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10])
            .unwrap();
        tmp.flush().unwrap();

        let mapped = MappedFile::open(tmp.path()).unwrap();
        assert_eq!(mapped.file_type(), Some(FileType::Jpeg));
    }

    #[test]
    fn nonexistent_file() {
        let result = MappedFile::open(Path::new("/nonexistent/path/file.bin"));
        assert!(result.is_err());
    }
}
