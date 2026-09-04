use crate::state::AppState;
use oxy_domain::{
    AssetTagAssignment, CustomTag, CustomTagId, MetadataPatch, TagDeleteImpact, TagSyncStatus,
};
use oxy_fs::FsCatalog;
use oxy_library::Library;
use oxy_metadata::MetadataFacade;
use std::{collections::HashSet, path::PathBuf, sync::Arc};
use tauri::State;

static TAG_SYNC_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn schedule_tag_xmp_sync(
    library: Arc<Library>,
    files: Arc<FsCatalog>,
    metadata: MetadataFacade,
) {
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = drain_tag_xmp_sync(&library, &files, &metadata) {
            eprintln!("failed to run tag XMP sync: {error}");
        }
    });
}

fn drain_tag_xmp_sync(
    library: &Library,
    files: &FsCatalog,
    metadata: &MetadataFacade,
) -> Result<(), String> {
    let _gate = TAG_SYNC_GATE
        .lock()
        .map_err(|_| "tag XMP sync lock poisoned".to_owned())?;
    for path in library
        .pending_tag_sync_paths()
        .map_err(|error| error.to_string())?
    {
        let result = sync_path(library, files, metadata, &path);
        if let Err(error) = result {
            library
                .fail_tag_xmp_sync(&path, &error)
                .map_err(|db_error| db_error.to_string())?;
        }
    }
    Ok(())
}

fn sync_path(
    library: &Library,
    files: &FsCatalog,
    metadata: &MetadataFacade,
    path: &std::path::Path,
) -> Result<(), String> {
    let asset = files.get_asset(path).map_err(|error| error.to_string())?;
    let current = metadata
        .read_metadata(path, asset.kind)
        .map_err(|error| error.to_string())?;
    let payload = library
        .tag_xmp_payload(path)
        .map_err(|error| error.to_string())?;
    let subjects = merge_managed_values(
        current.keywords,
        &payload.previous_subjects,
        &payload.subjects,
    );
    let hierarchical = merge_managed_values(
        current.hierarchical_keywords,
        &payload.previous_hierarchical,
        &payload.hierarchical,
    );
    metadata
        .patch_metadata(
            path,
            asset.kind,
            &MetadataPatch {
                keywords: Some(subjects),
                hierarchical_keywords: Some(hierarchical),
                ..MetadataPatch::default()
            },
        )
        .map_err(|error| error.to_string())?;
    library
        .complete_tag_xmp_sync(path, &payload.subjects, &payload.hierarchical)
        .map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        files.invalidate_directory(parent);
    }
    Ok(())
}

fn merge_managed_values(
    current: Vec<String>,
    previous_managed: &[String],
    desired: &[String],
) -> Vec<String> {
    let previous = previous_managed.iter().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    current
        .into_iter()
        .filter(|value| !previous.contains(value))
        .chain(desired.iter().cloned())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn start_sync(state: &AppState) {
    schedule_tag_xmp_sync(
        Arc::clone(&state.library),
        Arc::clone(&state.files),
        state.metadata.clone(),
    );
}

#[tauri::command]
pub(crate) fn list_custom_tags(state: State<'_, AppState>) -> Result<Vec<CustomTag>, String> {
    state
        .library
        .custom_tags()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_asset_tag_assignments(
    paths: Vec<PathBuf>,
    state: State<'_, AppState>,
) -> Result<Vec<AssetTagAssignment>, String> {
    state
        .library
        .asset_tag_assignments(&paths)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn create_custom_tag(
    parent_id: Option<CustomTagId>,
    name: String,
    state: State<'_, AppState>,
) -> Result<CustomTag, String> {
    state
        .library
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
        .library
        .update_custom_tag(id, parent_id, &name)
        .map_err(|error| error.to_string())?;
    start_sync(&state);
    Ok(tag)
}

#[tauri::command]
pub(crate) fn get_custom_tag_delete_impact(
    id: CustomTagId,
    state: State<'_, AppState>,
) -> Result<TagDeleteImpact, String> {
    state
        .library
        .custom_tag_delete_impact(id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn delete_custom_tag(
    id: CustomTagId,
    state: State<'_, AppState>,
) -> Result<TagDeleteImpact, String> {
    let impact = state
        .library
        .delete_custom_tag(id)
        .map_err(|error| error.to_string())?;
    start_sync(&state);
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
        .library
        .set_asset_tag(&paths, tag_id, assigned)
        .map_err(|error| error.to_string())?;
    start_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) fn get_tag_sync_status(state: State<'_, AppState>) -> Result<TagSyncStatus, String> {
    state
        .library
        .tag_sync_status()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn retry_tag_xmp_sync(state: State<'_, AppState>) {
    start_sync(&state);
}

#[cfg(test)]
mod tests {
    use super::merge_managed_values;

    #[test]
    fn replaces_only_values_managed_by_oxyviewer() {
        assert_eq!(
            merge_managed_values(
                vec!["external".into(), "old".into()],
                &["old".into()],
                &["new".into()]
            ),
            ["external", "new"]
        );
    }
}
