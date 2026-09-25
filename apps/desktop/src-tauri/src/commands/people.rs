use crate::state::AppState;
use oxy_domain::{
    AssetSummary, ConfirmFolderPerson, CreatePersonInstance, FolderPerson, HistoricalPerson,
    LinkHistoricalPerson, PersonInstance, PersonReview, PersonTagLink, PersonTagOverride,
    SetPersonReview, SetPersonTagLink, SetPersonTagOverride, UnlinkHistoricalPerson,
};
use oxy_fs::FsCatalog;
use std::path::{Path, PathBuf};
use tauri::State;

fn canonical_folder(path: &Path) -> Result<PathBuf, String> {
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    if !canonical.is_dir() {
        return Err("folder is unavailable".into());
    }
    Ok(canonical)
}

fn source_revision(
    files: &FsCatalog,
    folder: &Path,
    asset: &Path,
) -> Result<(PathBuf, String), String> {
    let canonical = asset.canonicalize().map_err(|error| error.to_string())?;
    if canonical.parent() != Some(folder) {
        return Err("asset is outside the selected folder".into());
    }
    let summary = files
        .get_asset(&canonical)
        .map_err(|error| error.to_string())?;
    Ok((
        canonical,
        format!("{}:{}", summary.size_bytes, summary.modified_at_ms),
    ))
}

fn stable_source_identity(
    files: &FsCatalog,
    folder: &Path,
    asset: &Path,
    expected_cheap_revision: &str,
) -> Result<String, String> {
    let observation = oxy_fs::observe_file(asset).map_err(|error| error.to_string())?;
    if observation.canonical_path != asset {
        return Err("asset path changed during review".into());
    }
    let (_, after) = source_revision(files, folder, asset)?;
    if after != expected_cheap_revision {
        return Err("source version changed; reload this photo".into());
    }
    Ok(observation.revision_id())
}

#[tauri::command]
pub(crate) async fn list_historical_people(
    state: State<'_, AppState>,
) -> Result<Vec<HistoricalPerson>, String> {
    let people = state.people.clone();
    tauri::async_runtime::spawn_blocking(move || {
        people
            .list_historical_people()
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn get_historical_link(
    folder_path: PathBuf,
    subject_id: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let people = state.people.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&folder_path)?;
        people
            .get_historical_link(&folder, &subject_id)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn get_person_tag_link(
    folder_path: PathBuf,
    subject_id: String,
    state: State<'_, AppState>,
) -> Result<Option<PersonTagLink>, String> {
    let people = state.people.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&folder_path)?;
        people
            .get_person_tag_link(&folder, &subject_id)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn set_person_tag_link(
    input: SetPersonTagLink,
    state: State<'_, AppState>,
) -> Result<PersonTagLink, String> {
    let people = state.people.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&input.folder_path)?;
        people
            .set_person_tag_link(&SetPersonTagLink {
                folder_path: folder,
                ..input
            })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn get_person_tag_override(
    folder_path: PathBuf,
    subject_id: String,
    asset_path: PathBuf,
    state: State<'_, AppState>,
) -> Result<Option<PersonTagOverride>, String> {
    let people = state.people.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&folder_path)?;
        people
            .get_person_tag_override(&folder, &subject_id, &asset_path)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn set_person_tag_override(
    input: SetPersonTagOverride,
    state: State<'_, AppState>,
) -> Result<PersonTagOverride, String> {
    let people = state.people.clone();
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&input.folder_path)?;
        let (asset, _) = source_revision(&files, &folder, &input.asset_path)?;
        people
            .set_person_tag_override(&SetPersonTagOverride {
                folder_path: folder,
                asset_path: asset,
                ..input
            })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn get_folder_person_reference_asset(
    folder_path: PathBuf,
    subject_id: String,
    state: State<'_, AppState>,
) -> Result<Option<AssetSummary>, String> {
    let people = state.people.clone();
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&folder_path)?;
        let person = people
            .list_folder_people(&folder)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|person| person.id == subject_id)
            .ok_or_else(|| "folder person not found".to_string())?;
        let Some(reference_id) = person.reference_instance_id else {
            return Ok(None);
        };
        let instance = people
            .get_person_instance(&folder, &reference_id)
            .map_err(|error| error.to_string())?;
        let (path, revision) = source_revision(&files, &folder, &instance.asset_path)?;
        if revision != instance.source_revision {
            return Ok(None);
        }
        files
            .get_asset(path)
            .map(Some)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn get_historical_reference_asset(
    historical_person_id: String,
    state: State<'_, AppState>,
) -> Result<Option<AssetSummary>, String> {
    let people = state.people.clone();
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let person = people
            .list_historical_people()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|person| person.id == historical_person_id)
            .ok_or_else(|| "historical person not found".to_string())?;
        let Ok(path) = person.reference_asset_path.canonicalize() else {
            return Ok(None);
        };
        let Ok(summary) = files.get_asset(&path) else {
            return Ok(None);
        };
        if format!("{}:{}", summary.size_bytes, summary.modified_at_ms)
            != person.reference_source_revision
        {
            return Ok(None);
        }
        Ok(Some(summary))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn link_historical_person(
    input: LinkHistoricalPerson,
    state: State<'_, AppState>,
) -> Result<HistoricalPerson, String> {
    let people = state.people.clone();
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&input.folder_path)?;
        let person = people
            .list_folder_people(&folder)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|person| person.id == input.subject_id)
            .ok_or_else(|| "folder person not found".to_string())?;
        let reference_id = person
            .reference_instance_id
            .ok_or_else(|| "confirmed reference missing".to_string())?;
        let instance = people
            .get_person_instance(&folder, &reference_id)
            .map_err(|error| error.to_string())?;
        let (_, revision) = source_revision(&files, &folder, &instance.asset_path)?;
        if revision != instance.source_revision {
            return Err("reference photo changed; review it again".into());
        }
        if let Some(history_id) = &input.historical_person_id {
            let history = people
                .list_historical_people()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|person| &person.id == history_id)
                .ok_or_else(|| "historical person not found".to_string())?;
            let summary = files
                .get_asset(&history.reference_asset_path)
                .map_err(|error| error.to_string())?;
            if format!("{}:{}", summary.size_bytes, summary.modified_at_ms)
                != history.reference_source_revision
            {
                return Err("historical reference changed; review it again".into());
            }
        }
        people
            .link_historical_person(&LinkHistoricalPerson {
                folder_path: folder,
                ..input
            })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn unlink_historical_person(
    input: UnlinkHistoricalPerson,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let people = state.people.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&input.folder_path)?;
        people
            .unlink_historical_person(&UnlinkHistoricalPerson {
                folder_path: folder,
                ..input
            })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn list_folder_people(
    folder_path: PathBuf,
    state: State<'_, AppState>,
) -> Result<Vec<FolderPerson>, String> {
    let people = state.people.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&folder_path)?;
        people
            .list_folder_people(&folder)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn create_folder_person(
    folder_path: PathBuf,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<FolderPerson, String> {
    let people = state.people.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&folder_path)?;
        people
            .create_folder_person(&folder, &request_id)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn confirm_folder_person(
    input: ConfirmFolderPerson,
    state: State<'_, AppState>,
) -> Result<FolderPerson, String> {
    let people = state.people.clone();
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&input.folder_path)?;
        let instance = people
            .get_person_instance(&folder, &input.reference_instance_id)
            .map_err(|error| error.to_string())?;
        let (_, revision) = source_revision(&files, &folder, &instance.asset_path)?;
        if instance.source_revision != revision {
            return Err("reference photo changed; review it again".into());
        }
        let input = ConfirmFolderPerson {
            folder_path: folder,
            ..input
        };
        people
            .confirm_folder_person(&input)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn create_person_instance(
    input: CreatePersonInstance,
    state: State<'_, AppState>,
) -> Result<PersonInstance, String> {
    let people = state.people.clone();
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&input.folder_path)?;
        let (asset, revision) = source_revision(&files, &folder, &input.asset_path)?;
        if input.source_revision != revision {
            return Err("source version changed; reload this photo".into());
        }
        let identity_revision = stable_source_identity(&files, &folder, &asset, &revision)?;
        let input = CreatePersonInstance {
            folder_path: folder,
            asset_path: asset,
            ..input
        };
        people
            .create_person_instance_with_source_identity(&input, Some(&identity_revision))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn list_person_instances(
    folder_path: PathBuf,
    asset_path: PathBuf,
    state: State<'_, AppState>,
) -> Result<Vec<PersonInstance>, String> {
    let people = state.people.clone();
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&folder_path)?;
        let (asset, _) = source_revision(&files, &folder, &asset_path)?;
        people
            .list_person_instances(&folder, &asset)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn list_person_reviews(
    folder_path: PathBuf,
    subject_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<PersonReview>, String> {
    let people = state.people.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&folder_path)?;
        people
            .list_person_reviews(&folder, &subject_id)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn set_person_review(
    input: SetPersonReview,
    state: State<'_, AppState>,
) -> Result<PersonReview, String> {
    let people = state.people.clone();
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&input.folder_path)?;
        let instance = people
            .get_person_instance(&folder, &input.instance_id)
            .map_err(|error| error.to_string())?;
        let (_, revision) = source_revision(&files, &folder, &instance.asset_path)?;
        if instance.source_revision != revision {
            return Err("source photo changed; review the instance again".into());
        }
        let input = SetPersonReview {
            folder_path: folder,
            ..input
        };
        people
            .set_person_review(&input)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn update_person_instance(
    input: oxy_domain::UpdatePersonInstance,
    state: State<'_, AppState>,
) -> Result<PersonInstance, String> {
    let people = state.people.clone();
    let files = state.files.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&input.folder_path)?;
        let instance = people
            .get_person_instance(&folder, &input.instance_id)
            .map_err(|e| e.to_string())?;
        let (_, revision) = source_revision(&files, &folder, &instance.asset_path)?;
        if revision != input.source_revision {
            return Err("source version changed; reload this photo".into());
        }
        let identity_revision =
            stable_source_identity(&files, &folder, &instance.asset_path, &revision)?;
        people
            .update_person_instance_with_source_identity(
                &oxy_domain::UpdatePersonInstance {
                    folder_path: folder,
                    ..input
                },
                Some(&identity_revision),
            )
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn reset_folder_person(
    input: oxy_domain::ResetFolderPerson,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let people = state.people.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = canonical_folder(&input.folder_path)?;
        people
            .reset_folder_person(&oxy_domain::ResetFolderPerson {
                folder_path: folder,
                ..input
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
