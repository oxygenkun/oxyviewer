use crate::state::AppState;
use oxy_domain::{
    AssetTagAssignment, AssetTagAssignmentsByPath, CustomTag, CustomTagId, MetadataPatch,
    TagDeleteImpact, TagSyncStatus,
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
pub(crate) async fn get_asset_tag_assignments(
    paths: Vec<PathBuf>,
    state: State<'_, AppState>,
) -> Result<Vec<AssetTagAssignment>, String> {
    let library = Arc::clone(&state.library);
    tauri::async_runtime::spawn_blocking(move || {
        library
            .asset_tag_assignments(&paths)
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
    let library = Arc::clone(&state.library);
    tauri::async_runtime::spawn_blocking(move || {
        library
            .asset_tag_assignments_by_path(&paths)
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
    state.people.capture_tag_paths()?;
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
    #[test]
    fn confirmed_person_writes_standard_xmp_keywords_and_updates_them() {
        use super::*;
        use oxy_domain::{FaceDecision, FaceObservation, NormalizedRect};
        let directory = tempfile::tempdir().unwrap();
        let asset = directory.path().join("portrait.jpg");
        std::fs::write(
            &asset,
            include_bytes!("../../../../../crates/oxy-faces/tests/data/fixtures/two_people.jpg"),
        )
        .unwrap();
        let library = Arc::new(Library::in_memory().unwrap());
        let people = crate::state::people::PeopleService::load(
            Arc::clone(&library),
            directory.path().join("people.json"),
        )
        .unwrap();
        let metadata = MetadataFacade::new(None);
        let files = FsCatalog::default();
        metadata
            .patch_metadata(
                &asset,
                oxy_domain::AssetKind::Jpeg,
                &MetadataPatch {
                    keywords: Some(vec!["external".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
        library
            .replace_asset_faces(
                &asset,
                "asset",
                "revision",
                "detector",
                "embedder",
                &[oxy_library::StoredFace {
                    observation: FaceObservation {
                        observation_id: "face".into(),
                        asset_id: "asset".into(),
                        asset_path: asset.clone(),
                        source_revision: "revision".into(),
                        local_index: 0,
                        bbox: NormalizedRect {
                            x: 0.1,
                            y: 0.1,
                            width: 0.2,
                            height: 0.2,
                        },
                        landmarks: vec![],
                        detection_score: 0.99,
                        detector_fingerprint: "detector".into(),
                    },
                    embedding: vec![1.0, 0.0],
                    face_pixels: 100,
                    clarity: 1.0,
                }],
            )
            .unwrap();
        people.create_person("alice", "Alice", None).unwrap();
        people
            .set_person_tag_path("alice", "人物|家人|Alice")
            .unwrap();
        people
            .decide(
                "face",
                &FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
            )
            .unwrap();
        library.refresh_person_tag_projection().unwrap();
        drain_tag_xmp_sync(&library, &files, &metadata).unwrap();
        let read = metadata
            .read_metadata(&asset, oxy_domain::AssetKind::Jpeg)
            .unwrap();
        assert!(read.keywords.contains(&"Alice".into()));
        assert!(read.keywords.contains(&"external".into()));
        assert_eq!(read.hierarchical_keywords, ["人物|家人|Alice"]);
        let xml = std::fs::read_to_string(oxy_fs::sidecar_path(&asset)).unwrap();
        assert!(xml.contains("dc:subject") && xml.contains("lr:hierarchicalSubject"));
        people.rename_person("alice", "Alicia").unwrap();
        library.refresh_person_tag_projection().unwrap();
        drain_tag_xmp_sync(&library, &files, &metadata).unwrap();
        let read = metadata
            .read_metadata(&asset, oxy_domain::AssetKind::Jpeg)
            .unwrap();
        assert!(!read.keywords.contains(&"Alice".into()));
        assert_eq!(read.hierarchical_keywords, ["人物|家人|Alicia"]);
        people.clear_decision("face").unwrap();
        library.refresh_person_tag_projection().unwrap();
        drain_tag_xmp_sync(&library, &files, &metadata).unwrap();
        let read = metadata
            .read_metadata(&asset, oxy_domain::AssetKind::Jpeg)
            .unwrap();
        assert_eq!(read.keywords, ["external"]);
        assert!(read.hierarchical_keywords.is_empty());
    }
}
