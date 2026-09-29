//! Real-fixture Release verification of the same media/domain adapters as IPC.
use oxy_domain::{
    BeginPersonAnalysis, FeatureModality, PersonAnalysisTask, PersonOperationState,
    PersonOperationStatus,
};
use oxy_people::{
    People,
    environment::{catalog, directml},
    execution::{
        enrollment::{self, AnalysisTaskSink},
        folder,
        operations::{PersonOperation, PersonOperations},
    },
    features::PersonFeature,
    inference::face::OnnxFaceModels,
};
use std::{path::PathBuf, sync::Arc, time::Instant};

fn operation() -> Arc<PersonOperation> {
    PersonOperations::default()
        .reserve(PersonOperationStatus {
            operation_id: "bench".into(),
            folder_path: None,
            state: PersonOperationState::Preparing,
            completed: 0,
            total: 0,
            detail: String::new(),
            error: None,
            run: None,
        })
        .unwrap()
}

fn serial(
    people: &People,
    files: &oxy_fs::FsCatalog,
    session: &str,
    request: &BeginPersonAnalysis,
    models: &mut OnnxFaceModels,
    media: &oxy_media::AnalysisInputService,
    work: &PersonOperation,
) {
    struct Sink<'a>(&'a People);
    impl AnalysisTaskSink for Sink<'_> {
        type Error = oxy_people::PeopleError;
        fn enqueue(
            &mut self,
            id: &str,
            tasks: &[oxy_domain::PersonAnalysisTaskInput],
        ) -> Result<usize, Self::Error> {
            self.0.enqueue_person_analysis_tasks(id, tasks)
        }
    }
    let manifest = catalog::manifest();
    let run = people.begin_person_analysis(request).unwrap();
    enrollment::enroll_folder_snapshot(files, session, &run, &manifest, &mut Sink(people), || {
        false
    })
    .unwrap();
    people.seal_person_analysis_tasks(&run.run_id).unwrap();
    let detector_fingerprint = oxy_people::stage_fingerprint(&manifest, catalog::DETECTOR).unwrap();
    let feature_space = manifest
        .stages
        .iter()
        .find(|s| s.stage_id == catalog::ENCODER)
        .unwrap()
        .feature_space_id
        .clone()
        .unwrap();
    let mut current = None;
    while people.person_analysis_is_current(&run.run_id).unwrap() {
        let Some(task) = people.claim_person_analysis_task(&run.run_id).unwrap() else {
            break;
        };
        if task.stage_id == catalog::DETECTOR {
            let pixels = prepare(files, media, &task, work).pixels;
            let detections = models
                .detect_for_cache(
                    &pixels,
                    &task.source_revision,
                    &task.stage_fingerprint,
                    0.5,
                    0.4,
                )
                .unwrap();
            people
                .complete_person_analysis_task_with_detections(&task, &[], Some(&detections))
                .unwrap();
            current = Some(pixels);
        } else {
            let detections = people
                .list_person_detections(
                    &task.folder_path,
                    &task.asset_path,
                    &task.source_revision,
                    &detector_fingerprint,
                )
                .unwrap();
            let mut features = Vec::new();
            for detection in detections {
                let values = models
                    .encode_detected_face(current.as_ref().unwrap(), &detection)
                    .unwrap();
                features.push(PersonFeature {
                    folder_path: task.folder_path.clone(),
                    asset_path: task.asset_path.clone(),
                    instance_id: detection.instance_id,
                    source_revision: task.source_revision.clone(),
                    feature_space_id: feature_space.clone(),
                    modality: FeatureModality::Face,
                    producer_fingerprint: task.stage_fingerprint.clone(),
                    pipeline_fingerprint: run.pipeline_fingerprint.clone(),
                    values,
                });
            }
            people
                .complete_person_analysis_task(&task, &features)
                .unwrap();
            current = None;
        }
    }
    let finished = people.get_person_analysis(&run.run_id).unwrap();
    assert_eq!(finished.failed_tasks, 0);
    assert_eq!(finished.completed_tasks, finished.total_tasks);
}

fn prepare(
    files: &oxy_fs::FsCatalog,
    media: &oxy_media::AnalysisInputService,
    task: &PersonAnalysisTask,
    work: &PersonOperation,
) -> oxy_media::AnalysisInput {
    let asset = files.get_asset(&task.asset_path).unwrap();
    let source = oxy_media::SourceRevision::observe(&task.asset_path).unwrap();
    media
        .prepare(
            &task.asset_path,
            asset.kind,
            &source,
            catalog::INPUT_REQUIREMENT,
            work.cancellation(),
        )
        .unwrap()
}

#[test]
#[ignore = "real installed ORT DirectML, model files and JPEG folder; Release throughput verification"]
fn directml_pipeline_real_folder() {
    let root = PathBuf::from(std::env::var_os("OXY_TEST_MODEL_ROOT").unwrap());
    let source = PathBuf::from(std::env::var_os("OXY_BENCH_IMAGE_DIR").unwrap());
    let scratch = tempfile::tempdir().unwrap();
    let photos = scratch.path().join("photos");
    std::fs::create_dir(&photos).unwrap();
    let sources = oxy_fs::scan_assets_with_progress(&source, |_| {}).unwrap();
    let mut jpegs: Vec<_> = sources
        .into_iter()
        .filter(|asset| {
            asset
                .path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("jpg"))
        })
        .collect();
    jpegs.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(jpegs.len() >= 16);
    for index in 0..16 {
        let path = &jpegs[index * (jpegs.len() - 1) / 15].path;
        std::fs::copy(path, photos.join(path.file_name().unwrap())).unwrap();
    }
    let files = oxy_fs::FsCatalog::default();
    let session = files.open_folder(&photos).unwrap();
    let folder_path = photos.canonicalize().unwrap();
    let library = oxy_library::Library::open(&scratch.path().join("test.sqlite")).unwrap();
    let people = People::new(library.store());
    let media = oxy_media::AnalysisInputService::new(&scratch.path().join("cache")).unwrap();
    let work = operation();
    let started = Instant::now();
    let mut models = directml::load_models(&root, &work).unwrap();
    eprintln!("ORT DirectML model setup: {:?}", started.elapsed());
    let manifest = catalog::manifest();
    let fingerprint = oxy_people::pipeline_fingerprint(&manifest).unwrap();
    let mut results = Vec::new();
    for round in 0..3 {
        for parallel in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let work = operation();
            let mode = if parallel { "pipeline" } else { "serial" };
            let request = BeginPersonAnalysis {
                folder_path: folder_path.clone(),
                pipeline_id: manifest.pipeline_id.clone(),
                pipeline_fingerprint: fingerprint.clone(),
                request_id: format!("{mode}-{round}"),
            };
            let started = Instant::now();
            if parallel {
                folder::analyse_folder(
                    &people,
                    &files,
                    &session.id,
                    &request,
                    &mut models,
                    &work,
                    |task, _| Ok(prepare(&files, &media, task, &work).pixels),
                )
                .unwrap();
                let status = work.snapshot().unwrap();
                assert_eq!(status.completed, 32);
                assert_eq!(status.total, 32);
                assert_eq!(status.run.unwrap().failed_tasks, 0);
            } else {
                serial(
                    &people,
                    &files,
                    &session.id,
                    &request,
                    &mut models,
                    &media,
                    &work,
                );
            }
            let ms = started.elapsed().as_secs_f64() * 1000.;
            eprintln!("{mode} round={round}: {ms:.2} ms / 16 photos, persisted 32 steps");
            let producer = oxy_people::stage_fingerprint(&manifest, catalog::DETECTOR).unwrap();
            let mut counts = Vec::new();
            for asset in oxy_fs::scan_assets_with_progress(&folder_path, |_| {}).unwrap() {
                let revision = oxy_fs::observe_file(&asset.path).unwrap().revision_id();
                counts.push(
                    people
                        .list_person_detections(&folder_path, &asset.path, &revision, &producer)
                        .unwrap()
                        .len(),
                );
            }
            results.push(
                serde_json::json!({"mode":mode,"round":round,"elapsed_ms":ms,"face_counts":counts}),
            );
        }
    }
    let expected = &results[0]["face_counts"];
    for result in &results {
        assert_eq!(&result["face_counts"], expected);
    }
    let output = PathBuf::from(std::env::var_os("OXY_BENCH_OUTPUT").unwrap());
    std::fs::write(output, serde_json::to_vec_pretty(&results).unwrap()).unwrap();
}
