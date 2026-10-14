use crate::state::AppState;
use oxy_core::contracts::RawDecoderStatus;
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub(crate) async fn get_raw_decoder_status(
    path: Option<PathBuf>,
    refresh: bool,
    state: State<'_, AppState>,
) -> Result<RawDecoderStatus, String> {
    let service = state.resource_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.raw_decoder_status(path, refresh))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn retry_raw_full(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let service = state.resource_service.clone();
    tauri::async_runtime::spawn_blocking(move || service.retry_raw_full(path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn open_raw_decoder_install_page(web: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || open_install_page(web))
        .await
        .map_err(|error| error.to_string())?
}

fn open_install_page(web: bool) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        use windows::{
            Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
            core::{PCWSTR, w},
        };
        // Fixed official targets only; the frontend cannot launch arbitrary URLs or executables.
        let open = |url: &str| {
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
            (result.0 as isize) > 32
        };
        if !web && open("ms-windows-store://pdp/?ProductId=9NCTDW2W1BH8") {
            return Ok("store".into());
        }
        if open("https://apps.microsoft.com/detail/9nctdw2w1bh8") {
            return Ok("web".into());
        }
        Err("Could not open Microsoft Store or its official web page".into())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = web;
        Err("Windows RAW extension is available only on Windows".into())
    }
}
