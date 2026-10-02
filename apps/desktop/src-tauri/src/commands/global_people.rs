use crate::state::AppState;
use oxy_domain::*;
use std::{path::PathBuf, sync::Arc};
use tauri::State;

#[tauri::command]
pub(crate) async fn get_global_person_folders(
    person_id: String,
    offset: u32,
    limit: u32,
    state: State<'_, AppState>,
) -> Result<GlobalPersonFolders, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .global_person_folders(&person_id, offset, limit)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn get_global_person_gallery(
    person_id: String,
    offset: u32,
    limit: u32,
    state: State<'_, AppState>,
) -> Result<GlobalPersonGallery, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .global_person_gallery(&person_id, offset, limit)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn set_global_person_tag(
    input: SetGlobalPersonTag,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .set_global_person_tag(&input)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn get_global_person_references(
    person_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<PersonTuple>, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .global_person_reference_tuples(&person_id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn save_person_tuple_geometry(
    input: SavePersonTupleGeometry,
    state: State<'_, AppState>,
) -> Result<PersonTuple, String> {
    let people = Arc::clone(&state.people);
    let files = Arc::clone(&state.files);
    tauri::async_runtime::spawn_blocking(move || {
        let folder = input
            .folder_path
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let observation = oxy_fs::observe_file(&input.asset_path).map_err(|e| e.to_string())?;
        let asset = files
            .get_asset(&observation.canonical_path)
            .map_err(|e| e.to_string())?;
        if input.source_revision != format!("{}:{}", asset.size_bytes, asset.modified_at_ms) {
            return Err("源图已变化，请重新打开照片".into());
        }
        if oxy_fs::observe_file(&asset.path).map_err(|e| e.to_string())? != observation {
            return Err("源图已变化".into());
        }
        people
            .save_person_tuple_geometry(
                &SavePersonTupleGeometry {
                    folder_path: folder,
                    asset_path: asset.path,
                    ..input
                },
                &observation.revision_id(),
            )
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn list_global_people(
    state: State<'_, AppState>,
) -> Result<Vec<GlobalPerson>, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || people.global_people().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn get_folder_people_workspace(
    folder_path: PathBuf,
    state: State<'_, AppState>,
) -> Result<FolderPeopleWorkspace, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        let path = folder_path.canonicalize().map_err(|e| e.to_string())?;
        people
            .folder_people_workspace(&path)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn save_global_person(
    input: SaveGlobalPerson,
    state: State<'_, AppState>,
) -> Result<GlobalPerson, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people.save_global_person(&input).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn review_person_tuples(
    input: ReviewPersonTuples,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .review_person_tuples(&input)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn set_global_person_reference(
    input: SetGlobalPersonReference,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .set_global_person_reference(&input)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn get_person_tuple_asset(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<AssetSummary, String> {
    let files = Arc::clone(&state.files);
    tauri::async_runtime::spawn_blocking(move || files.get_asset(&path).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) fn start_people_grouping(
    session_id: String,
    input: RunPeopleGrouping,
    refresh_features: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let operation = super::people_analysis::reserve(
        &state,
        input.request_id.clone(),
        Some(input.folder_path.clone()),
        PersonOperationState::Clustering,
    )?;
    let people = Arc::clone(&state.people);
    let files = Arc::clone(&state.files);
    let work = Arc::clone(&operation);
    let root = state.person_model_dir.clone();
    let preview_dir = state.cache.preview_dir();
    super::people_analysis::launch(operation, move || {
        let folder = files
            .session_directory(&session_id, Some(&input.folder_path))
            .map_err(|e| e.to_string())?;
        if refresh_features
            || !people
                .has_current_person_features(&folder)
                .map_err(|e| e.to_string())?
        {
            #[cfg(all(windows, target_arch = "x86_64"))]
            {
                use oxy_people::environment::catalog;
                let manifest = catalog::manifest();
                work.update(|s| {
                    s.state = PersonOperationState::Preparing;
                    s.detail = "正在准备当前文件夹的人脸特征…".into();
                });
                let media = oxy_media::AnalysisInputService::new(&preview_dir)
                    .map_err(|e| e.to_string())?;
                let mut models = oxy_people::environment::directml::load_models(&root, &work)?;
                oxy_people::execution::folder::analyse_folder(
                    &people,
                    &files,
                    &session_id,
                    &BeginPersonAnalysis {
                        folder_path: folder.clone(),
                        pipeline_id: manifest.pipeline_id.clone(),
                        pipeline_fingerprint: oxy_people::pipeline_fingerprint(&manifest)
                            .map_err(|e| e.to_string())?,
                        request_id: format!("{}:features", input.request_id),
                    },
                    &mut models,
                    &work,
                    |task, requirement| {
                        let asset = files
                            .get_asset(&task.asset_path)
                            .map_err(|e| e.to_string())?;
                        let source = oxy_media::SourceRevision::observe(&task.asset_path)
                            .map_err(|e| e.to_string())?;
                        if source.revision_id != task.source_revision {
                            return Err("源图已变化，请重新识别".into());
                        }
                        media
                            .prepare(
                                &task.asset_path,
                                asset.kind,
                                &source,
                                requirement,
                                work.cancellation(),
                            )
                            .map(|i| i.pixels)
                            .map_err(|e| e.to_string())
                    },
                )?;
            }
            #[cfg(not(all(windows, target_arch = "x86_64")))]
            {
                let _ = (&root, &preview_dir);
                return Err("此平台尚未配置识别运行时；已有特征仍可分组与检索".into());
            }
        }
        work.update(|s| {
            s.state = PersonOperationState::Clustering;
            s.total = 0;
            s.completed = 0;
            s.detail = if input.person_ids.is_some() {
                "正在寻找指定人物…"
            } else {
                "正在自动分组并匹配全局人物…"
            }
            .into();
        });
        let result = people
            .run_people_grouping(
                &RunPeopleGrouping {
                    folder_path: folder,
                    ..input
                },
                work.cancellation(),
            )
            .map_err(|e| e.to_string())?;
        work.update(|s| {
            s.detail = format!(
                "完成：未知身份 {} 组，已知身份 {} 人；请核对实例归属。",
                result.unknown_count, result.known_count
            );
        });
        Ok(())
    });
    Ok(())
}
