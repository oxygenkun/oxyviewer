//! Person use cases, model operations and application-owned analysis workers.
use super::{
    People,
    environment::catalog as model_catalog,
    execution::operations::{PersonOperation, PersonOperations},
    types::*,
};
use crate::{
    browse::types::AssetSummary, files::FsCatalog, resources::cache::CacheManager,
    runtime::CoreRuntime,
};
use std::{path::PathBuf, sync::Arc};

pub struct PersonService {
    pub(crate) people: Arc<People>,
    pub(crate) files: Arc<FsCatalog>,
    pub(crate) person_operations: Arc<PersonOperations>,
    pub(crate) runtime: CoreRuntime,
    pub(crate) person_model_dir: PathBuf,
    pub(crate) cache: Arc<CacheManager>,
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod analysis_verification;

impl PersonService {
    pub fn set_global_person_tag(&self, input: SetGlobalPersonTag) -> Result<(), String> {
        crate::app::workflows::person_tags::set_global_person_tag(&self.people.store, &input)
            .map_err(|error| error.to_string())
    }

    pub fn workspace(&self, folder_path: PathBuf) -> Result<FolderPeopleWorkspace, String> {
        let folder = folder_path.canonicalize().map_err(|e| e.to_string())?;
        self.people
            .folder_people_workspace(&folder)
            .map_err(|e| e.to_string())
    }

    pub fn tuple_asset(&self, path: PathBuf) -> Result<AssetSummary, String> {
        self.files.get_asset(&path).map_err(|e| e.to_string())
    }

    pub fn save_tuple_geometry(
        &self,
        input: SavePersonTupleGeometry,
    ) -> Result<PersonTuple, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let folder = input
            .folder_path
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let observation =
            crate::files::observe_file(&input.asset_path).map_err(|e| e.to_string())?;
        let asset = self.tuple_asset(observation.canonical_path.clone())?;
        if input.source_revision != format!("{}:{}", asset.size_bytes, asset.modified_at_ms) {
            return Err("源图已变化，请重新打开照片".into());
        }
        if crate::files::observe_file(&asset.path).map_err(|e| e.to_string())? != observation {
            return Err("源图已变化".into());
        }
        self.people
            .save_person_tuple_geometry(
                &SavePersonTupleGeometry {
                    folder_path: folder,
                    asset_path: asset.path,
                    ..input
                },
                &observation.revision_id(),
            )
            .map_err(|e| e.to_string())
    }

    pub fn detections(
        &self,
        folder_path: PathBuf,
        asset_path: PathBuf,
    ) -> Result<PersonDetectionSnapshot, String> {
        let people = Arc::clone(&self.people);

        let folder = folder_path
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let observation =
            crate::files::observe_file(&asset_path).map_err(|error| error.to_string())?;
        let revision = observation.revision_id();
        let producer =
            crate::people::stage_fingerprint(&model_catalog::manifest(), model_catalog::DETECTOR)
                .map_err(|error| error.to_string())?;
        let instances = people
            .list_person_detections(&folder, &observation.canonical_path, &revision, &producer)
            .map_err(|error| error.to_string())?;
        Ok(PersonDetectionSnapshot {
            source_revision: revision,
            instances,
        })
    }

    pub fn adopt_detection(
        &self,
        folder_path: PathBuf,
        asset_path: PathBuf,
        source_revision: String,
        instance_id: String,
    ) -> Result<PersonInstance, String> {
        let people = Arc::clone(&self.people);
        let files = Arc::clone(&self.files);

        let folder = folder_path
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let before = crate::files::observe_file(&asset_path).map_err(|error| error.to_string())?;
        if before.revision_id() != source_revision {
            return Err("源图已变化，请重新识别".into());
        }
        let producer =
            crate::people::stage_fingerprint(&model_catalog::manifest(), model_catalog::DETECTOR)
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
        if crate::files::observe_file(&asset.path)
            .map_err(|error| error.to_string())?
            .revision_id()
            != source_revision
        {
            return Err("源图已变化".into());
        }
        people
            .create_person_instance_with_source_identity(
                &CreatePersonInstance {
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
    }

    pub fn clusters(&self, folder_path: PathBuf) -> Result<Option<PersonClusterSnapshot>, String> {
        let people = Arc::clone(&self.people);

        let folder = folder_path.canonicalize().map_err(|e| e.to_string())?;
        people.person_clusters(&folder).map_err(|e| e.to_string())
    }

    pub fn adopt_cluster(
        &self,
        input: AdoptPersonCluster,
    ) -> Result<AdoptPersonClusterResult, String> {
        let people = Arc::clone(&self.people);

        people
            .adopt_person_cluster(&input)
            .map_err(|e| e.to_string())
    }

    fn reserve(
        &self,
        request_id: String,
        folder: Option<PathBuf>,
        mode: PersonOperationState,
    ) -> Result<Arc<PersonOperation>, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        if request_id.trim().is_empty() {
            return Err("请求编号不能为空".into());
        }
        self.person_operations.reserve(PersonOperationStatus {
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

    pub fn start_clustering(
        &self,
        session_id: String,
        folder_path: PathBuf,
        request_id: String,
    ) -> Result<(), String> {
        let operation = self.reserve(
            request_id,
            Some(folder_path.clone()),
            PersonOperationState::Clustering,
        )?;
        let people = Arc::clone(&self.people);
        let files = Arc::clone(&self.files);
        let work = Arc::clone(&operation);
        PersonOperations::launch(&self.runtime, operation, move || {
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
        })?;
        Ok(())
    }

    pub fn start_model_download(&self, model_id: String, request_id: String) -> Result<(), String> {
        if ![model_catalog::DETECTOR, model_catalog::ENCODER, "runtime"]
            .contains(&model_id.as_str())
        {
            return Err("未知模型".into());
        }
        let operation = self.reserve(request_id, None, PersonOperationState::Downloading)?;
        let root = self.person_model_dir.clone();
        let work = Arc::clone(&operation);
        PersonOperations::launch(&self.runtime, operation, move || {
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
                crate::people::environment::ort::download(&root, &mut progress)?;
                #[cfg(not(all(windows, target_arch = "x86_64")))]
                return Err("此平台尚未提供运行时安装".into());
            } else {
                crate::people::environment::artifacts::download_artifact(
                    &model_catalog::manifest(),
                    &model_id,
                    &root,
                    &mut progress,
                )
                .map_err(|error| error.to_string())?;
            }
            work.update(|status| status.detail = "下载完成，文件校验通过。".into());
            Ok(())
        })?;
        Ok(())
    }

    pub fn import_model(
        &self,
        model_id: String,
        path: PathBuf,
        request_id: String,
    ) -> Result<(), String> {
        if ![model_catalog::DETECTOR, model_catalog::ENCODER].contains(&model_id.as_str()) {
            return Err("未知模型".into());
        }
        let operation = self.reserve(request_id, None, PersonOperationState::Preparing)?;
        let root = self.person_model_dir.clone();
        let work = Arc::clone(&operation);
        PersonOperations::launch(&self.runtime, operation, move || {
            let input = std::fs::File::open(path).map_err(|error| error.to_string())?;
            crate::people::environment::artifacts::install_artifact_reader(
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
        })?;
        Ok(())
    }

    pub fn models(&self) -> Vec<PersonModelStatus> {
        let root = self.person_model_dir.clone();

        let mut models = model_catalog::model_status(&root);
        #[cfg(all(windows, target_arch = "x86_64"))]
        {
            let installed = crate::people::environment::ort::installed(&root);
            models.push(PersonModelStatus {
                id: "runtime".into(),
                name: "ORT DirectML 运行时".into(),
                size_bytes: crate::people::environment::ort::SIZE,
                installed: matches!(installed, Ok(Some(_))),
                download_available: true,
                source_url: crate::people::environment::ort::SOURCE_URL.into(),
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
    }

    pub fn operation(&self) -> Result<Option<PersonOperationStatus>, String> {
        self.person_operations.snapshot()
    }
    pub fn cancel_operation(&self, id: &str) -> Result<(), String> {
        self.person_operations.cancel(id)
    }
    pub fn start_analysis(
        &self,
        session_id: String,
        folder_path: PathBuf,
        request_id: String,
    ) -> Result<(), String> {
        self.start_grouping(
            session_id,
            RunPeopleGrouping {
                folder_path,
                person_ids: None,
                similarity: 0.4,
                request_id,
            },
            true,
        )
    }
    pub fn start_grouping(
        &self,
        session_id: String,
        input: RunPeopleGrouping,
        refresh_features: bool,
    ) -> Result<(), String> {
        let operation = self.reserve(
            input.request_id.clone(),
            Some(input.folder_path.clone()),
            PersonOperationState::Clustering,
        )?;
        let people = Arc::clone(&self.people);
        let files = Arc::clone(&self.files);
        let work = Arc::clone(&operation);
        let root = self.person_model_dir.clone();
        let preview_dir = self.cache.preview_dir();
        PersonOperations::launch(&self.runtime, operation, move || {
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
                    use crate::people::environment::catalog;
                    let manifest = catalog::manifest();
                    work.update(|s| {
                        s.state = PersonOperationState::Preparing;
                        s.detail = "正在准备当前文件夹的人脸特征…".into();
                    });
                    let media = oxy_media::AnalysisInputService::new(&preview_dir)
                        .map_err(|e| e.to_string())?;
                    let mut models =
                        crate::people::environment::directml::load_models(&root, &work)?;
                    crate::people::execution::folder::analyse_folder(
                        &people,
                        &files,
                        &session_id,
                        &BeginPersonAnalysis {
                            folder_path: folder.clone(),
                            pipeline_id: manifest.pipeline_id.clone(),
                            pipeline_fingerprint: crate::people::pipeline_fingerprint(&manifest)
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
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppPaths, OxyApp};
    use std::{sync::mpsc, time::Duration};

    fn host(path: &std::path::Path) -> OxyApp {
        OxyApp::open(AppPaths {
            data_dir: path.join("data"),
            preview_dir: path.join("cache"),
        })
        .unwrap()
        .0
    }

    #[test]
    fn geometry_use_case_validates_source_and_folder_before_writing() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let photo = root.join("photo.jpg");
        std::fs::write(&photo, b"source").unwrap();
        let app = host(&root);
        let asset = app.person_service.tuple_asset(photo.clone()).unwrap();
        let mut input = SavePersonTupleGeometry {
            folder_path: root.clone(),
            asset_path: photo.clone(),
            instance_id: None,
            expected_revision: 0,
            source_revision: format!("{}:{}", asset.size_bytes, asset.modified_at_ms),
            face_box: Some([0.1, 0.1, 0.2, 0.2]),
            body_box: None,
            request_id: "geometry".into(),
        };
        let tuple = app
            .person_service
            .save_tuple_geometry(input.clone())
            .unwrap();
        assert_eq!(tuple.asset_path, photo);
        assert!(
            app.person_service
                .workspace(root.clone())
                .unwrap()
                .groups
                .iter()
                .flat_map(|g| &g.members)
                .any(|t| t.id == tuple.id)
        );
        let other = root.join("other");
        std::fs::create_dir(&other).unwrap();
        input.folder_path = other;
        input.request_id = "outside-folder".into();
        assert!(
            app.person_service
                .save_tuple_geometry(input.clone())
                .is_err()
        );
        input.folder_path = root.clone();
        input.request_id = "stale-source".into();
        std::fs::write(&photo, b"changed source").unwrap();
        assert!(
            app.person_service
                .save_tuple_geometry(input.clone())
                .is_err()
        );
        assert_eq!(
            app.people
                .list_person_instances(&root, &photo)
                .unwrap()
                .len(),
            1
        );
        assert!(app.shutdown(Duration::from_secs(3)));
        input.request_id = "after-stop".into();
        assert!(
            app.person_service
                .save_tuple_geometry(input)
                .unwrap_err()
                .contains("shutting down")
        );
    }

    #[test]
    fn invalid_requests_and_shutdown_do_not_reserve_a_new_operation() {
        let directory = tempfile::tempdir().unwrap();
        let app = host(directory.path());
        assert!(
            app.person_service
                .start_model_download("unknown".into(), "request".into())
                .is_err()
        );
        assert!(
            app.person_service
                .start_model_download(model_catalog::DETECTOR.into(), " ".into())
                .is_err()
        );
        assert!(app.person_service.operation().unwrap().is_none());
        assert!(app.shutdown(Duration::from_secs(3)));
        assert!(
            app.person_service
                .start_analysis(
                    "session".into(),
                    directory.path().to_owned(),
                    "after-stop".into()
                )
                .is_err()
        );
        assert!(app.person_service.operation().unwrap().is_none());
    }

    #[test]
    fn cancelling_an_operation_does_not_admit_another_until_its_worker_exits() {
        let directory = tempfile::tempdir().unwrap();
        let app = host(directory.path());
        let operation = app
            .person_service
            .reserve("held".into(), None, PersonOperationState::Preparing)
            .unwrap();
        let (entered, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        PersonOperations::launch(&app.runtime, operation.clone(), move || {
            entered.send(()).unwrap();
            released.recv().unwrap();
            Ok(())
        })
        .unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        app.person_service.cancel_operation("held").unwrap();
        assert!(operation.is_cancelled());
        assert!(
            app.person_service
                .start_model_download(model_catalog::DETECTOR.into(), "second".into())
                .is_err()
        );
        assert!(
            app.person_service
                .import_model(
                    model_catalog::ENCODER.into(),
                    directory.path().join("missing.onnx"),
                    "third".into()
                )
                .is_err()
        );
        assert_eq!(
            app.person_service
                .operation()
                .unwrap()
                .unwrap()
                .operation_id,
            "held"
        );
        release.send(()).unwrap();
        assert!(app.shutdown(Duration::from_secs(3)));
        assert_eq!(
            app.person_service.operation().unwrap().unwrap().state,
            PersonOperationState::Cancelled
        );
    }

    #[test]
    fn adopting_a_stale_detection_creates_no_manual_fact() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let photo = root.join("photo.jpg");
        std::fs::write(&photo, b"source").unwrap();
        let app = host(&root);
        let before = app
            .person_service
            .detections(root.clone(), photo.clone())
            .unwrap();
        assert!(before.instances.is_empty());
        std::fs::write(&photo, b"changed source").unwrap();
        assert!(
            app.person_service
                .adopt_detection(
                    root.clone(),
                    photo.clone(),
                    before.source_revision,
                    "missing".into()
                )
                .is_err()
        );
        assert!(
            app.people
                .list_person_instances(&root, &photo)
                .unwrap()
                .is_empty()
        );
        assert!(app.shutdown(Duration::from_secs(3)));
    }
}
