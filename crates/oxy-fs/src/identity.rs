//! Application path recovery and the shared media source observation.
pub use oxy_media::source::{FileObservation, observe_file};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Resolve a user-selected recovery directory without recursively scanning it.
pub fn relocation_directory(path: &Path) -> io::Result<PathBuf> {
    let canonical = path.canonicalize()?;
    fs::read_dir(&canonical)?;
    Ok(canonical)
}
