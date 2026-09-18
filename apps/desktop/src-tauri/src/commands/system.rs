use crate::state::AppState;
use oxy_domain::{DebugQueueSnapshot, FaceAssetReveal, FaceWorkbenchContext, PerfScenario};
use std::{
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
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
pub(crate) fn get_media_resource_stats() -> Result<oxy_domain::ResourceRegistryStats, String> {
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
    let preview = state.preview_queue.clone();
    let metadata = state.metadata_queue.clone();
    let directory_tree = state.directory_tree_queue.clone();
    let library_index = state.library_index_queue.clone();
    let snapshots = std::sync::Arc::clone(&state.debug_snapshots);
    let requested_at = Instant::now();
    tauri::async_runtime::spawn_blocking(move || {
        let collection_started = Instant::now();
        let worker_wait_micros = requested_at
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX);
        let captured_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        let [loupe, thumbnail] = preview.debug_snapshot();
        let (queues, stale_queues) = snapshots.remember([
            ("loupe", loupe),
            ("thumbnail", thumbnail),
            ("metadata", metadata.debug_snapshot()),
            ("directoryTree", directory_tree.debug_snapshot()),
            ("libraryIndex", library_index.debug_snapshot()),
        ]);
        Ok(DebugQueueSnapshot {
            captured_at_unix_ms,
            worker_wait_micros,
            collection_micros: collection_started
                .elapsed()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX),
            queues,
            stale_queues,
        })
    })
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

/// Window label of the face workbench, and the events that keep the two windows
/// in step. Every cross-window message goes through a command rather than a
/// frontend `emit`, so the Rust/TypeScript IPC contract test can see it.
pub(crate) const FACE_WORKBENCH_WINDOW_LABEL: &str = "face-workbench";
const FACE_WORKBENCH_VISIBILITY_EVENT: &str = "face-workbench-visibility";
const FACE_WORKBENCH_CONTEXT_EVENT: &str = "face-workbench-context";
const FACE_WORKBENCH_CONTEXT_REQUEST_EVENT: &str = "face-workbench-context-request";
const FACE_ASSET_REVEAL_EVENT: &str = "face-asset-reveal";

/// Builds the workbench window hidden. Building is idempotent, so opening an
/// already-created window only shows it.
fn create_face_workbench_window(app: &AppHandle) -> tauri::Result<()> {
    if app
        .get_webview_window(FACE_WORKBENCH_WINDOW_LABEL)
        .is_some()
    {
        return Ok(());
    }
    let dev_url = app.config().build.dev_url.clone().map(|mut url| {
        url.set_query(Some("workbench=faces"));
        url
    });
    let webview_url = match dev_url {
        Some(url) => WebviewUrl::External(url),
        None => WebviewUrl::App("index.html".into()),
    };
    WebviewWindowBuilder::new(app, FACE_WORKBENCH_WINDOW_LABEL, webview_url)
        .title("OxyViewer Face Workbench")
        .inner_size(1320.0, 880.0)
        .min_inner_size(960.0, 640.0)
        .visible(false)
        .build()
        .map(|_| ())
}

#[tauri::command]
pub(crate) fn open_face_workbench_window(app: AppHandle) -> Result<(), String> {
    create_face_workbench_window(&app).map_err(|error| error.to_string())?;
    let window = app
        .get_webview_window(FACE_WORKBENCH_WINDOW_LABEL)
        .ok_or_else(|| "face workbench window is unavailable".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    app.emit(FACE_WORKBENCH_VISIBILITY_EVENT, true)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn close_face_workbench_window(app: AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window(FACE_WORKBENCH_WINDOW_LABEL) else {
        return Ok(());
    };
    window.hide().map_err(|error| error.to_string())?;
    app.emit(FACE_WORKBENCH_VISIBILITY_EVENT, false)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn is_face_workbench_window_open(app: AppHandle) -> bool {
    app.get_webview_window(FACE_WORKBENCH_WINDOW_LABEL)
        .is_some_and(|window| window.is_visible().unwrap_or(false))
}

/// Pushes the main window's browse scope to the workbench. The workbench cannot
/// read the main window's store, so the scope travels as published state.
#[tauri::command]
pub(crate) fn publish_face_workbench_context(
    app: AppHandle,
    context: FaceWorkbenchContext,
) -> Result<(), String> {
    app.emit(FACE_WORKBENCH_CONTEXT_EVENT, context)
        .map_err(|error| error.to_string())
}

/// A freshly mounted workbench asks once; the main window answers with the same
/// publisher it uses for changes.
#[tauri::command]
pub(crate) fn request_face_workbench_context(app: AppHandle) -> Result<(), String> {
    app.emit(FACE_WORKBENCH_CONTEXT_REQUEST_EVENT, ())
        .map_err(|error| error.to_string())
}

/// Clicking a face asks the main window to show the photo it came from.
#[tauri::command]
pub(crate) fn notify_face_asset_reveal(
    app: AppHandle,
    reveal: FaceAssetReveal,
) -> Result<(), String> {
    app.emit(FACE_ASSET_REVEAL_EVENT, reveal)
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
