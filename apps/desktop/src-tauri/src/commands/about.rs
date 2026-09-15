//! About panel commands: build metadata, manual release check, and the fixed
//! external destinations that panel may open.

use crate::providers::updates;
use oxy_domain::{AboutLink, AppInfo, UpdateStatus};
#[cfg(unix)]
use std::process::Command;
use tauri::AppHandle;

/// The project author shown in Settings → About.
///
/// Compiled in by `build.rs` from `bundle.publisher`: Tauri generates the
/// runtime `BundleConfig` without that key, and a packaged app ships no
/// `tauri.conf.json`, so the config file cannot be read at run time.
const AUTHOR: &str = env!("OXYVIEWER_ABOUT_AUTHOR");

/// Config values can be missing or blank; the panel only shows real ones.
fn trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Build metadata for the About settings panel.
///
/// The version is read from the running package instead of being duplicated in
/// the frontend, so it cannot drift from the shipped bundle.
#[tauri::command]
pub(crate) fn get_app_info(app: AppHandle) -> AppInfo {
    let config = app.config();
    AppInfo {
        name: config
            .product_name
            .clone()
            .unwrap_or_else(|| "OxyViewer".to_owned()),
        version: app.package_info().version.to_string(),
        repository_url: updates::REPOSITORY_URL.to_owned(),
        author: trimmed(AUTHOR),
        license: config.bundle.license.as_deref().and_then(trimmed),
    }
}

/// Checks GitHub Releases for a newer published release.
///
/// The request runs on a blocking worker because `ureq` is synchronous, and it
/// only happens when the user asks for it.
#[tauri::command]
pub(crate) async fn check_for_updates(app: AppHandle) -> Result<UpdateStatus, String> {
    let current_version = app.package_info().version.to_string();
    tauri::async_runtime::spawn_blocking(move || updates::check_for_updates(&current_version))
        .await
        .map_err(|error| error.to_string())?
}

/// Opens one of the About panel's fixed destinations in the system browser.
///
/// `release_url` is honored only when it points at an OxyViewer release page, so
/// the frontend cannot launch arbitrary URLs.
#[tauri::command]
pub(crate) async fn open_about_link(
    target: AboutLink,
    release_url: Option<String>,
) -> Result<(), String> {
    let url = updates::about_url(target, release_url.as_deref())?;
    tauri::async_runtime::spawn_blocking(move || open_external_url(&url))
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(target_os = "windows")]
fn open_external_url(url: &str) -> Result<(), String> {
    use windows::{
        Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
        core::{PCWSTR, w},
    };
    let url = url.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(url.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if (result.0 as isize) > 32 {
        Ok(())
    } else {
        Err("could not open the link in the default browser".to_owned())
    }
}

#[cfg(target_os = "macos")]
fn open_external_url(url: &str) -> Result<(), String> {
    Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not open the link in the default browser: {error}"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_external_url(url: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not open the link in the default browser: {error}"))
}

#[cfg(not(any(windows, unix)))]
fn open_external_url(_url: &str) -> Result<(), String> {
    Err("opening links is unsupported on this platform".to_owned())
}
