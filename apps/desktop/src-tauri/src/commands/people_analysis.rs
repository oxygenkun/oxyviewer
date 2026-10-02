use crate::state::AppState;
use oxy_domain::{PersonModelStatus, PersonOperationState, PersonOperationStatus};
use oxy_people::{environment::catalog as model_catalog, execution::operations::PersonOperation};
use std::{path::PathBuf, sync::Arc};
use tauri::State;

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests;

#[tauri::command]
pub(crate) async fn get_person_detections(
    folder_path: PathBuf,
    asset_path: PathBuf,
    state: State<'_, AppState>,
) -> Result<oxy_domain::PersonDetectionSnapshot, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        let folder = folder_path
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let observation = oxy_fs::observe_file(&asset_path).map_err(|error| error.to_string())?;
        let revision = observation.revision_id();
        let producer =
            oxy_people::stage_fingerprint(&model_catalog::manifest(), model_catalog::DETECTOR)
                .map_err(|error| error.to_string())?;
        let instances = people
            .list_person_detections(&folder, &observation.canonical_path, &revision, &producer)
            .map_err(|error| error.to_string())?;
        Ok(oxy_domain::PersonDetectionSnapshot {
            source_revision: revision,
            instances,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn adopt_person_detection(
    folder_path: PathBuf,
    asset_path: PathBuf,
    source_revision: String,
    instance_id: String,
    state: State<'_, AppState>,
) -> Result<oxy_domain::PersonInstance, String> {
    let people = Arc::clone(&state.people);
    let files = Arc::clone(&state.files);
    tauri::async_runtime::spawn_blocking(move || {
        let folder = folder_path
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let before = oxy_fs::observe_file(&asset_path).map_err(|error| error.to_string())?;
        if before.revision_id() != source_revision {
            return Err("源图已变化，请重新识别".into());
        }
        let producer =
            oxy_people::stage_fingerprint(&model_catalog::manifest(), model_catalog::DETECTOR)
                .map_err(|error| error.to_string())?;
        let instance = people
            .list_person_detections(&folder, &before.canonical_path, &source_revision, &producer)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|item| item.instance_id == instance_id)
            .ok_or("检测结果已失效")?;
        let asset = files
            .get_asset(&before.canonical_path)
            .map_err(|error| error.to_string())?;
        if oxy_fs::observe_file(&asset.path)
            .map_err(|error| error.to_string())?
            .revision_id()
            != source_revision
        {
            return Err("源图已变化".into());
        }
        people
            .create_person_instance_with_source_identity(
                &oxy_domain::CreatePersonInstance {
                    folder_path: folder.clone(),
                    asset_path: asset.path.clone(),
                    source_revision: format!("{}:{}", asset.size_bytes, asset.modified_at_ms),
                    face_box: instance.face_box,
                    body_box: instance.body_box,
                    request_id: format!(
                        "adopt:{}:{}:{source_revision}:{instance_id}",
                        folder.display(),
                        asset.path.display()
                    ),
                },
                Some(&source_revision),
            )
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(super) fn reserve(
    state: &AppState,
    request_id: String,
    folder: Option<PathBuf>,
    mode: PersonOperationState,
) -> Result<Arc<PersonOperation>, String> {
    if request_id.trim().is_empty() {
        return Err("请求编号不能为空".into());
    }
    state.person_operations.reserve(PersonOperationStatus {
        operation_id: request_id,
        folder_path: folder,
        state: mode,
        completed: 0,
        total: 0,
        detail: "准备中…".into(),
        error: None,
        run: None,
    })
}

#[tauri::command]
pub(crate) async fn get_person_clusters(
    folder_path: PathBuf,
    state: State<'_, AppState>,
) -> Result<Option<oxy_domain::PersonClusterSnapshot>, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        let folder = folder_path.canonicalize().map_err(|e| e.to_string())?;
        people.person_clusters(&folder).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) fn start_person_clustering(
    session_id: String,
    folder_path: PathBuf,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let operation = reserve(
        &state,
        request_id,
        Some(folder_path.clone()),
        PersonOperationState::Clustering,
    )?;
    let people = Arc::clone(&state.people);
    let files = Arc::clone(&state.files);
    let work = Arc::clone(&operation);
    launch(operation, move || {
        let folder = files
            .session_directory(&session_id, Some(&folder_path))
            .map_err(|e| e.to_string())?;
        work.update(|s| s.detail = "正在使用已有特征聚类…".into());
        let snapshot = people
            .cluster_folder(&folder, work.cancellation())
            .map_err(|e| e.to_string())?;
        work.update(|s| {
            s.detail = format!(
                "聚类完成：{} 个分组，{} 个未分组人脸。",
                snapshot.clusters.len(),
                snapshot.ungrouped.len()
            );
        });
        Ok(())
    });
    Ok(())
}

#[tauri::command]
pub(crate) async fn adopt_person_cluster(
    input: oxy_domain::AdoptPersonCluster,
    state: State<'_, AppState>,
) -> Result<oxy_domain::AdoptPersonClusterResult, String> {
    let people = Arc::clone(&state.people);
    tauri::async_runtime::spawn_blocking(move || {
        people
            .adopt_person_cluster(&input)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(super) fn launch(
    operation: Arc<PersonOperation>,
    action: impl FnOnce() -> Result<(), String> + Send + 'static,
) {
    tauri::async_runtime::spawn(async move {
        let result = tauri::async_runtime::spawn_blocking(action).await;
        operation.finish(result.unwrap_or_else(|error| Err(error.to_string())));
    });
}

#[tauri::command]
pub(crate) async fn get_person_models(
    state: State<'_, AppState>,
) -> Result<Vec<PersonModelStatus>, String> {
    let root = state.person_model_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut models = model_catalog::model_status(&root);
        #[cfg(all(windows, target_arch = "x86_64"))]
        {
            let installed = oxy_people::environment::ort::installed(&root);
            models.push(PersonModelStatus {
                id: "runtime".into(),
                name: "ORT DirectML 运行时".into(),
                size_bytes: oxy_people::environment::ort::SIZE,
                installed: matches!(installed, Ok(Some(_))),
                download_available: true,
                source_url: oxy_people::environment::ort::SOURCE_URL.into(),
                usage: "微软官方 ONNX Runtime，使用 DirectML GPU 推理。".into(),
                error: installed.err(),
            });
        }
        #[cfg(not(all(windows, target_arch = "x86_64")))]
        models.push(PersonModelStatus {
            id: "runtime".into(),
            name: "人物推理运行时".into(),
            size_bytes: 0,
            installed: false,
            download_available: false,
            source_url: String::new(),
            usage: "此平台的运行时安装尚未提供。".into(),
            error: None,
        });
        models
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_person_operation(
    state: State<'_, AppState>,
) -> Result<Option<PersonOperationStatus>, String> {
    state.person_operations.snapshot()
}

#[tauri::command]
pub(crate) fn cancel_person_operation(
    operation_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.person_operations.cancel(&operation_id)
}

#[tauri::command]
pub(crate) fn start_person_model_download(
    model_id: String,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if ![model_catalog::DETECTOR, model_catalog::ENCODER, "runtime"].contains(&model_id.as_str()) {
        return Err("未知模型".into());
    }
    let operation = reserve(&state, request_id, None, PersonOperationState::Downloading)?;
    let root = state.person_model_dir.clone();
    let work = Arc::clone(&operation);
    launch(operation, move || {
        let mut progress = |completed, total| {
            work.update(|status| {
                status.completed = completed;
                status.total = total;
                status.detail = "正在下载并校验…".into();
            });
            !work.is_cancelled()
        };
        if model_id == "runtime" {
            #[cfg(all(windows, target_arch = "x86_64"))]
            oxy_people::environment::ort::download(&root, &mut progress)?;
            #[cfg(not(all(windows, target_arch = "x86_64")))]
            return Err("此平台尚未提供运行时安装".into());
        } else {
            oxy_people::environment::artifacts::download_artifact(
                &model_catalog::manifest(),
                &model_id,
                &root,
                &mut progress,
            )
            .map_err(|error| error.to_string())?;
        }
        work.update(|status| status.detail = "下载完成，文件校验通过。".into());
        Ok(())
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn import_person_model(
    model_id: String,
    path: PathBuf,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if ![model_catalog::DETECTOR, model_catalog::ENCODER].contains(&model_id.as_str()) {
        return Err("未知模型".into());
    }
    let operation = reserve(&state, request_id, None, PersonOperationState::Preparing)?;
    let root = state.person_model_dir.clone();
    let work = Arc::clone(&operation);
    launch(operation, move || {
        let input = std::fs::File::open(path).map_err(|error| error.to_string())?;
        oxy_people::environment::artifacts::install_artifact_reader(
            &model_catalog::manifest(),
            &model_id,
            input,
            &root,
            &mut |completed, total| {
                work.update(|status| {
                    status.completed = completed;
                    status.total = total;
                    status.detail = "正在导入并校验…".into();
                });
                !work.is_cancelled()
            },
        )
        .map_err(|error| error.to_string())?;
        work.update(|status| status.detail = "模型导入成功。".into());
        Ok(())
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn start_folder_person_analysis(
    session_id: String,
    folder_path: PathBuf,
    request_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    super::global_people::start_people_grouping(
        session_id,
        oxy_domain::RunPeopleGrouping {
            folder_path,
            person_ids: None,
            similarity: 0.4,
            request_id,
        },
        true,
        state,
    )
}
