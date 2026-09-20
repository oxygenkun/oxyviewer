use oxy_domain::BurstGroup;
use std::path::PathBuf;
use tauri::State;

use crate::state::AppState;

/// Groups the given assets into continuous-shooting bursts.
///
/// The caller passes its whole current listing, in browse order, and the
/// scanner parses only the paths it has not seen. Reading maker notes therefore
/// never happens on the folder open path, and each call costs only the files
/// of the newest page.
#[tauri::command]
pub(crate) async fn scan_burst_groups(
    paths: Vec<PathBuf>,
    state: State<'_, AppState>,
) -> Result<Vec<BurstGroup>, String> {
    let scanner = state.burst_scan.clone();
    tauri::async_runtime::spawn_blocking(move || scanner.scan(&paths))
        .await
        .map_err(|error| format!("burst scan failed: {error}"))
}
