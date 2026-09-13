//! OS launch boundary. Call from a blocking worker; never pass preview cache paths.
use oxy_domain::ExternalOpenResult;
use std::{borrow::Cow, io, path::Path, process::Command};

pub fn validate_application(path: &Path) -> io::Result<()> {
    if !path.is_absolute() || !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Application not found: {}", path.display()),
        ));
    }
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Choose an .exe application",
        ));
    }
    Ok(())
}

fn validate_file(path: &Path) -> io::Result<()> {
    if !path.is_absolute() || !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Image file not found: {}", path.display()),
        ));
    }
    Ok(())
}

pub fn open_with_application(application: &Path, path: &Path) -> io::Result<ExternalOpenResult> {
    validate_application(application)?;
    validate_file(path)?;
    let mut command = Command::new(application);
    command.arg(application_path(path).as_ref());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash.
    }
    let mut child = command.spawn()?;
    // Reap on Unix too, without keeping an IPC invocation alive for the editor lifetime.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(ExternalOpenResult::Launched)
}

pub fn open_with_system_dialog(path: &Path) -> io::Result<ExternalOpenResult> {
    validate_file(path)?;
    #[cfg(windows)]
    {
        windows_dialog(path)
    }
    #[cfg(not(windows))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "The system Open With dialog is currently available on Windows only",
        ))
    }
}

#[cfg(windows)]
fn windows_dialog(path: &Path) -> io::Result<ExternalOpenResult> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            Foundation::ERROR_CANCELLED,
            System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            UI::Shell::{OAIF_EXEC, OPENASINFO, SHOpenWithDialog},
        },
        core::{HRESULT, PCWSTR},
    };
    let path = application_path(path);
    if path.to_str().is_some_and(|path| path.starts_with(r"\\?\")) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows Open With cannot represent this file path; choose a configured application instead",
        ));
    }
    let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // A dedicated thread guarantees an STA even if the blocking pool uses MTA.
    std::thread::spawn(move || {
        // SAFETY: this fresh thread has no COM apartment. Balance successful init below.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() }.map_err(io::Error::other)?;
        let info = OPENASINFO {
            pcszFile: PCWSTR(file.as_ptr()),
            pcszClass: PCWSTR::null(),
            oaifInFlags: OAIF_EXEC,
        };
        // SAFETY: UTF-16 storage and OPENASINFO remain alive throughout the modal call.
        // No owner: a cross-thread owner joins input queues and lets the shell's
        // modal selection/slow application activation block the desktop window.
        // The chosen path is owned by this operation; browsing may continue safely.
        let result = unsafe { SHOpenWithDialog(None, &info) };
        // SAFETY: balances CoInitializeEx on the same thread after all shell work.
        unsafe { CoUninitialize() };
        match result {
            Ok(()) => Ok(ExternalOpenResult::Launched),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => {
                Ok(ExternalOpenResult::Cancelled)
            }
            Err(error) => Err(io::Error::other(error)),
        }
    })
    .join()
    .map_err(|_| io::Error::other("Open With dialog thread failed"))?
}

/// Shell applications commonly reject Rust's canonical verbatim Windows paths.
/// Only use a legacy spelling when opening it resolves to the very same target.
/// Blindly stripping the prefix can open a different file (e.g. trailing dots).
fn application_path(path: &Path) -> Cow<'_, Path> {
    #[cfg(windows)]
    if let Some(candidate) = legacy_path_candidate(path) {
        if let (Ok(original), Ok(legacy)) = (
            std::fs::canonicalize(path),
            std::fs::canonicalize(&candidate),
        ) {
            if original == legacy {
                return Cow::Owned(candidate);
            }
        }
    }
    Cow::Borrowed(path)
}

#[cfg(windows)]
fn legacy_path_candidate(path: &Path) -> Option<std::path::PathBuf> {
    let value = path.to_str()?;
    if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{unc}").into());
    }
    let disk = value.strip_prefix(r"\\?\")?;
    let bytes = disk.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
        .then(|| disk.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_or_relative_targets_before_launch() {
        assert!(validate_application(Path::new("relative.exe")).is_err());
        assert!(open_with_system_dialog(Path::new("missing image.jpg")).is_err());
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_file(dir.path()).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn uses_compatible_paths_without_redirecting_special_windows_names() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("照片 A.jpg");
        std::fs::write(&plain, b"original").unwrap();
        let canonical = std::fs::canonicalize(&plain).unwrap();
        assert_eq!(application_path(&canonical).as_ref(), plain);
        let special = std::fs::canonicalize(dir.path())
            .unwrap()
            .join("照片 A.jpg.");
        std::fs::write(&special, b"different original").unwrap();
        assert_eq!(application_path(&special).as_ref(), special);
        assert_eq!(
            legacy_path_candidate(Path::new(r"\\?\UNC\server\share\照片 A.jpg")).unwrap(),
            Path::new(r"\\server\share\照片 A.jpg")
        );
    }
}
