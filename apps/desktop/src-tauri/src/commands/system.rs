use crate::state::AppState;
use oxy_core::contracts::{DebugQueueSnapshot, PerfScenario};
use std::{path::PathBuf, time::Instant};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

#[tauri::command]
pub(crate) fn cancel_job(job_id: String, state: State<'_, AppState>) -> bool {
    state.jobs.cancel(&job_id)
}

/// Returns the performance scenario injected via `OXY_PERF_SCENARIO`, if any.
/// Used only by the end-to-end performance harness (docs/PERF_E2E.md).
#[tauri::command]
pub(crate) fn get_perf_scenario() -> Option<PerfScenario> {
    let raw = std::env::var("OXY_PERF_SCENARIO").ok()?;
    serde_json::from_str(&raw).ok()
}

/// Explicit diagnostics, available to debug builds and isolated perf runs.
#[tauri::command]
pub(crate) fn get_media_resource_stats()
-> Result<oxy_core::contracts::ResourceRegistryStats, String> {
    if !cfg!(debug_assertions) && get_perf_scenario().is_none() {
        return Err("resource diagnostics require an explicit performance scenario".into());
    }
    Ok(oxy_media::shared_resource_registry().stats())
}

/// Snapshot collection runs off the UI thread, including any queue contention.
#[tauri::command]
pub(crate) async fn get_debug_queue_snapshot(
    state: State<'_, AppState>,
) -> Result<DebugQueueSnapshot, String> {
    if !cfg!(debug_assertions) && get_perf_scenario().is_none() {
        return Err("queue diagnostics are only available in debug builds".into());
    }
    let diagnostics = state.queue_diagnostics.clone();
    let requested_at = Instant::now();
    tauri::async_runtime::spawn_blocking(move || diagnostics.collect(requested_at))
        .await
        .map_err(|error| error.to_string())?
}

pub(crate) fn create_debug_queue_window(app: &AppHandle) -> tauri::Result<()> {
    if !cfg!(debug_assertions) || app.get_webview_window("debug-queues").is_some() {
        return Ok(());
    }
    let debug_dev_url = app.config().build.dev_url.clone().map(|mut url| {
        url.set_query(Some("debug=queues"));
        url
    });
    let webview_url = match debug_dev_url {
        Some(url) => WebviewUrl::External(url),
        None => WebviewUrl::App("index.html".into()),
    };
    WebviewWindowBuilder::new(app, "debug-queues", webview_url)
        .title("OxyViewer Queue Observatory")
        .inner_size(1180.0, 760.0)
        .min_inner_size(760.0, 520.0)
        .visible(false)
        .build()
        .map(|_| ())
}

#[tauri::command]
pub(crate) fn open_debug_queue_window(app: AppHandle) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("queue diagnostics are only available in debug builds".into());
    }
    let window = app
        .get_webview_window("debug-queues")
        .ok_or_else(|| "queue diagnostics window is unavailable".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    window
        .emit("debug-queue-visibility", true)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn close_debug_queue_window(app: AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("debug-queues") else {
        return Ok(());
    };
    window.hide().map_err(|error| error.to_string())?;
    window
        .emit("debug-queue-visibility", false)
        .map_err(|error| error.to_string())
}

/// Writes the performance harness report JSON to a runner-owned path.
#[tauri::command]
pub(crate) fn write_perf_report(path: PathBuf, contents: String) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, contents).map_err(|error| error.to_string())
}
