use crate::state::AppState;
use oxy_core::contracts::{
    AssetTagAssignment, AssetTagAssignmentsByPath, CustomTag, CustomTagId, TagDeleteImpact,
    TagSyncStatus,
};
use std::{path::PathBuf, sync::Arc};
use tauri::State;

#[tauri::command]
pub(crate) fn list_custom_tags(state: State<'_, AppState>) -> Result<Vec<CustomTag>, String> {
    state.tags.custom_tags().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn get_asset_tag_assignments(
    paths: Vec<PathBuf>,
    state: State<'_, AppState>,
) -> Result<Vec<AssetTagAssignment>, String> {
    let tags = Arc::clone(&state.tags);
    tauri::async_runtime::spawn_blocking(move || {
        tags.asset_tag_assignments(&paths)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn get_asset_tag_assignments_by_path(
    paths: Vec<PathBuf>,
    state: State<'_, AppState>,
) -> Result<Vec<AssetTagAssignmentsByPath>, String> {
    let tags = Arc::clone(&state.tags);
    tauri::async_runtime::spawn_blocking(move || {
        tags.asset_tag_assignments_by_path(&paths)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn get_asset_tag_source_kinds(
    path: PathBuf,
    tag_id: CustomTagId,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let tags = Arc::clone(&state.tags);
    tauri::async_runtime::spawn_blocking(move || {
        tags.asset_tag_source_kinds(&path, tag_id)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) fn create_custom_tag(
    parent_id: Option<CustomTagId>,
    name: String,
    state: State<'_, AppState>,
) -> Result<CustomTag, String> {
    state
        .tags
        .create_custom_tag(parent_id, &name)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn update_custom_tag(
    id: CustomTagId,
    parent_id: Option<CustomTagId>,
    name: String,
    state: State<'_, AppState>,
) -> Result<CustomTag, String> {
    let tag = state
        .tags
        .update_custom_tag(id, parent_id, &name)
        .map_err(|error| error.to_string())?;
    state.tag_sync.schedule();
    Ok(tag)
}

#[tauri::command]
pub(crate) fn get_custom_tag_delete_impact(
    id: CustomTagId,
    state: State<'_, AppState>,
) -> Result<TagDeleteImpact, String> {
    state
        .tags
        .custom_tag_delete_impact(id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn delete_custom_tag(
    id: CustomTagId,
    state: State<'_, AppState>,
) -> Result<TagDeleteImpact, String> {
    let impact = state
        .tags
        .delete_custom_tag(id)
        .map_err(|error| error.to_string())?;
    state.tag_sync.schedule();
    Ok(impact)
}

#[tauri::command]
pub(crate) fn set_asset_custom_tag(
    paths: Vec<PathBuf>,
    tag_id: CustomTagId,
    assigned: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .tags
        .set_asset_tag(&paths, tag_id, assigned)
        .map_err(|error| error.to_string())?;
    state.tag_sync.schedule();
    Ok(())
}

#[tauri::command]
pub(crate) fn get_tag_sync_status(state: State<'_, AppState>) -> Result<TagSyncStatus, String> {
    state
        .tags
        .tag_sync_status()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn retry_tag_xmp_sync(state: State<'_, AppState>) {
    state.tag_sync.schedule();
}
