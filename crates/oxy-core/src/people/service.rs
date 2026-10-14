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
    pub(crate) tag_sync: Arc<crate::tags::xmp_sync::TagXmpSync>,
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod analysis_verification;

impl PersonService {
    fn admit_mutation(&self) -> Result<(), String> {
        if self.runtime.is_stopping() {
            Err("application is shutting down".into())
        } else {
            Ok(())
        }
    }

    pub fn catalog(&self) -> Result<Vec<GlobalPerson>, String> {
        self.people
            .global_people()
            .map_err(|error| error.to_string())
    }
    pub fn folders(
        &self,
        person: String,
        offset: u32,
        limit: u32,
    ) -> Result<GlobalPersonFolders, String> {
        self.people
            .global_person_folders(&person, offset, limit)
            .map_err(|error| error.to_string())
    }
    pub fn gallery(
        &self,
        person: String,
        offset: u32,
        limit: u32,
    ) -> Result<GlobalPersonGallery, String> {
        self.people
            .global_person_gallery(&person, offset, limit)
            .map_err(|error| error.to_string())
    }
    pub fn references(&self, person: String) -> Result<Vec<PersonTuple>, String> {
        self.people
            .global_person_reference_tuples(&person)
            .map_err(|error| error.to_string())
    }
    pub fn save_person(&self, input: SaveGlobalPerson) -> Result<GlobalPerson, String> {
        self.admit_mutation()?;
        self.people
            .save_global_person(&input)
            .map_err(|error| error.to_string())
    }
    pub fn set_reference(&self, input: SetGlobalPersonReference) -> Result<(), String> {
        self.admit_mutation()?;
        self.people
            .set_global_person_reference(&input)
            .map_err(|error| error.to_string())
    }
    pub fn review_tuples(&self, input: ReviewPersonTuples) -> Result<usize, String> {
        self.admit_mutation()?;
        let result =
            crate::app::workflows::person_review::review_person_tuples(&self.people.store, &input)
                .map_err(|error| error.to_string())?;
        self.tag_sync.schedule();
        Ok(result)
    }
    pub fn set_global_person_tag(&self, input: SetGlobalPersonTag) -> Result<(), String> {
        self.admit_mutation()?;
        crate::app::workflows::person_tags::set_global_person_tag(&self.people.store, &input)
            .map_err(|error| error.to_string())?;
        self.tag_sync.schedule();
        Ok(())
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
        self.admit_mutation()?;
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
        let result = crate::app::workflows::person_geometry::save_person_tuple_geometry(
            &self.people.store,
            &SavePersonTupleGeometry {
                folder_path: folder,
                asset_path: asset.path,
                ..input
            },
            &observation.revision_id(),
        )
        .map_err(|e| e.to_string())?;
        self.tag_sync.schedule();
        Ok(result)
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
    #[cfg(test)]
    fn start_analysis(
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
        input.folder_path = root;
        input.request_id = "stale-source".into();
        std::fs::write(&photo, b"changed source").unwrap();
        assert!(
            app.person_service
                .save_tuple_geometry(input.clone())
                .is_err()
        );
        assert_eq!(
            crate::people::repo::global::tuples(&app.person_service.people.store.read(), None)
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
    fn committed_person_review_wakes_the_owned_xmp_mirror_and_stop_rejects_mutations() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let photo = root.join("photo.jpg");
        image::RgbImage::new(8, 8).save(&photo).unwrap();
        let app = host(&root);
        let person = app
            .person_service
            .save_person(SaveGlobalPerson {
                id: None,
                display_name: "Ada".into(),
                expected_revision: 0,
                request_id: "person".into(),
            })
            .unwrap();
        let tag = app.tags.create_custom_tag(None, "Ada").unwrap();
        app.person_service
            .set_global_person_tag(SetGlobalPersonTag {
                person_id: person.id.clone(),
                tag_id: Some(tag.id),
                expected_revision: person.revision,
                request_id: "tag".into(),
            })
            .unwrap();
        let asset = app.person_service.tuple_asset(photo.clone()).unwrap();
        let tuple = app
            .person_service
            .save_tuple_geometry(SavePersonTupleGeometry {
                folder_path: root.clone(),
                asset_path: photo.clone(),
                instance_id: None,
                expected_revision: 0,
                source_revision: format!("{}:{}", asset.size_bytes, asset.modified_at_ms),
                face_box: Some([0.1, 0.1, 0.2, 0.2]),
                body_box: None,
                request_id: "geometry".into(),
            })
            .unwrap();
        let workspace = app.person_service.workspace(root.clone()).unwrap();
        let review = ReviewPersonTuples {
            folder_path: root,
            workspace_revision: workspace.revision,
            tuples: vec![tuple],
            person_id: person.id,
            decision: PersonReviewDecision::Belongs,
            replace_confirmed: false,
            request_id: "review".into(),
        };
        app.person_service.review_tuples(review.clone()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if app
                .metadata_service
                .metadata
                .read_metadata(&photo, oxy_media::types::AssetKind::Jpeg)
                .unwrap()
                .keywords
                == ["Ada"]
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "committed review did not reach its XMP mirror"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.shutdown(Duration::from_secs(3)));
        assert!(
            app.person_service
                .review_tuples(review)
                .unwrap_err()
                .contains("shutting down")
        );
        assert!(
            app.person_service
                .save_person(SaveGlobalPerson {
                    id: None,
                    display_name: "After stop".into(),
                    expected_revision: 0,
                    request_id: "stopped".into(),
                })
                .is_err()
        );
        assert_eq!(app.person_service.catalog().unwrap().len(), 1);
    }

    #[test]
    fn headless_review_commits_identity_and_tag_contribution_together() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let photo = root.join("photo.jpg");
        std::fs::write(&photo, b"source").unwrap();
        let app = host(&root);
        let asset = app.person_service.tuple_asset(photo.clone()).unwrap();
        let tuple = app
            .person_service
            .save_tuple_geometry(SavePersonTupleGeometry {
                folder_path: root.clone(),
                asset_path: photo.clone(),
                instance_id: None,
                expected_revision: 0,
                source_revision: format!("{}:{}", asset.size_bytes, asset.modified_at_ms),
                face_box: Some([0.1, 0.1, 0.2, 0.2]),
                body_box: None,
                request_id: "geometry".into(),
            })
            .unwrap();
        let person = app
            .person_service
            .people
            .save_global_person(&SaveGlobalPerson {
                id: None,
                display_name: "Ada".into(),
                expected_revision: 0,
                request_id: "person".into(),
            })
            .unwrap();
        let tag = app.tags.create_custom_tag(None, "Ada").unwrap();
        app.person_service
            .set_global_person_tag(SetGlobalPersonTag {
                person_id: person.id.clone(),
                tag_id: Some(tag.id),
                expected_revision: person.revision,
                request_id: "tag".into(),
            })
            .unwrap();
        let workspace = app.person_service.workspace(root.clone()).unwrap();
        let input = ReviewPersonTuples {
            folder_path: root.clone(),
            workspace_revision: workspace.revision.clone(),
            tuples: vec![tuple],
            person_id: person.id.clone(),
            decision: PersonReviewDecision::Belongs,
            replace_confirmed: false,
            request_id: "review".into(),
        };
        app.person_service
            .people
            .store
            .write()
            .execute_batch(
                "CREATE TRIGGER reject_review_event BEFORE INSERT ON global_person_events
             BEGIN SELECT RAISE(ABORT, 'event write failed'); END;",
            )
            .unwrap();
        assert!(app.person_service.review_tuples(input.clone()).is_err());
        assert_eq!(
            app.person_service.workspace(root.clone()).unwrap(),
            workspace
        );
        assert_eq!(
            app.tags
                .asset_tag_assignments(std::slice::from_ref(&photo))
                .unwrap()[0]
                .assigned_count,
            0
        );
        app.person_service
            .people
            .store
            .write()
            .execute_batch("DROP TRIGGER reject_review_event;")
            .unwrap();
        assert_eq!(app.person_service.review_tuples(input.clone()).unwrap(), 1);
        assert_eq!(app.person_service.review_tuples(input).unwrap(), 1);
        assert_eq!(
            app.tags
                .asset_tag_assignments(std::slice::from_ref(&photo))
                .unwrap()[0]
                .assigned_count,
            1
        );
        let confirmed = app.person_service.workspace(root).unwrap();
        assert!(
            confirmed
                .groups
                .iter()
                .flat_map(|group| &group.members)
                .any(|tuple| {
                    tuple.person_id.as_deref() == Some(&person.id)
                        && tuple.decision == Some(PersonReviewDecision::Belongs)
                })
        );
        assert!(app.shutdown(Duration::from_secs(3)));
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
    fn reviewing_a_stale_suggestion_creates_no_manual_fact() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let app = host(&root);
        let person = app
            .person_service
            .people
            .save_global_person(&SaveGlobalPerson {
                id: None,
                display_name: "Ada".into(),
                expected_revision: 0,
                request_id: "person".into(),
            })
            .unwrap();
        crate::people::clusters::tests::analyzed_fixture(&app.person_service.people, &root);
        let workspace = app
            .person_service
            .people
            .run_people_grouping(
                &RunPeopleGrouping {
                    folder_path: root.clone(),
                    person_ids: None,
                    similarity: 0.5,
                    request_id: "group".into(),
                },
                &crate::jobs::CancellationToken::default(),
            )
            .unwrap();
        let tuples = workspace.groups[0].members.clone();
        std::fs::write(&tuples[0].asset_path, b"changed source").unwrap();
        assert!(
            app.person_service
                .review_tuples(ReviewPersonTuples {
                    folder_path: root,
                    workspace_revision: workspace.revision,
                    tuples,
                    person_id: person.id,
                    decision: PersonReviewDecision::Belongs,
                    replace_confirmed: false,
                    request_id: "stale-review".into(),
                })
                .is_err()
        );
        assert!(
            crate::people::repo::global::tuples(&app.person_service.people.store.read(), None)
                .unwrap()
                .is_empty()
        );
        assert!(
            crate::people::repo::global::reviews(&app.person_service.people.store.read(), None)
                .unwrap()
                .is_empty()
        );
        assert!(app.shutdown(Duration::from_secs(3)));
    }
}
