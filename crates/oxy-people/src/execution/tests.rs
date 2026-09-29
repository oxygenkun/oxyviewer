use super::{
    operations::PersonOperations,
    persistence::{FaceOutput, InferredAsset, save},
    pipeline,
};
use crate::{People, testing};
use oxy_domain::{
    BeginPersonAnalysis, PersonAnalysisRun, PersonAnalysisState, PersonAnalysisTaskInput,
    PersonOperationState, PersonOperationStatus,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

fn operation(operations: &PersonOperations) -> Arc<super::operations::PersonOperation> {
    operations
        .reserve(PersonOperationStatus {
            operation_id: "test".into(),
            folder_path: None,
            state: PersonOperationState::Analysing,
            completed: 0,
            total: 0,
            detail: String::new(),
            error: None,
            run: None,
        })
        .unwrap()
}

fn enroll(people: &People, folder: &Path, request: &str) -> PersonAnalysisRun {
    let run = people
        .begin_person_analysis(&BeginPersonAnalysis {
            folder_path: folder.into(),
            pipeline_id: "test".into(),
            pipeline_fingerprint: "a".repeat(64),
            request_id: request.into(),
        })
        .unwrap();
    let mut tasks = Vec::new();
    for name in ["a.jpg", "b.jpg"] {
        let path = folder.join(name);
        std::fs::write(&path, b"source").unwrap();
        for (order, stage) in ["detect", "encode"].into_iter().enumerate() {
            tasks.push(PersonAnalysisTaskInput {
                asset_path: path.clone(),
                source_revision: oxy_fs::observe_file(&path).unwrap().revision_id(),
                stage_id: stage.into(),
                stage_fingerprint: if order == 0 { "b" } else { "c" }.repeat(64),
                stage_order: order as u32,
            });
        }
    }
    people
        .enqueue_person_analysis_tasks(&run.run_id, &tasks)
        .unwrap();
    people.seal_person_analysis_tasks(&run.run_id).unwrap()
}

#[test]
fn three_stages_really_overlap_and_preserve_order() {
    let rendezvous = AtomicUsize::new(0);
    let meet = || {
        rendezvous.fetch_add(1, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(3);
        while rendezvous.load(Ordering::SeqCst) < 3 {
            assert!(Instant::now() < deadline, "stages were serialized");
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    let mut next = 0;
    let mut saved = Vec::new();
    pipeline::execute(
        || {
            let value = next;
            next += 1;
            if value == 2 {
                meet();
            }
            Ok((value < 8).then_some(value))
        },
        |value| {
            if value == 1 {
                meet();
            }
            value
        },
        |value| {
            if value == 0 {
                meet();
            }
            saved.push(value);
            Ok(())
        },
        || false,
    )
    .unwrap();
    assert_eq!(saved, (0..8).collect::<Vec<_>>());
}

#[test]
fn save_failure_releases_blocked_upstream_workers() {
    let mut produced = 0;
    let result = pipeline::execute(
        || {
            produced += 1;
            Ok(Some(produced))
        },
        |value| value,
        |_| Err("disk failure".into()),
        || false,
    );
    assert_eq!(result, Err("disk failure".into()));
    assert!(produced <= 6, "unbounded preparation: {produced}");
}

#[test]
fn inference_panic_closes_channels_instead_of_hanging() {
    let mut next = 0;
    let result = pipeline::execute(
        || {
            next += 1;
            Ok(Some(next))
        },
        |_| -> usize { panic!("test native adapter failure") },
        |_| Ok(()),
        || false,
    );
    assert!(result.is_err());
}

#[test]
fn cancellation_inside_inference_discards_queued_results() {
    let cancel = AtomicBool::new(false);
    let mut saved = 0;
    pipeline::execute(
        || Ok(Some(1)),
        |value| {
            cancel.store(true, Ordering::Release);
            value
        },
        |_| {
            saved += 1;
            Ok(())
        },
        || cancel.load(Ordering::Acquire),
    )
    .unwrap();
    assert_eq!(saved, 0);
}

#[test]
fn preparation_failure_does_not_prevent_other_assets_from_finishing() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().canonicalize().unwrap();
    let people = testing::in_memory();
    let run = enroll(&people, &folder, "begin");
    let operations = PersonOperations::default();
    let work = operation(&operations);
    pipeline::execute(
        || {
            people
                .claim_person_analysis_stage(&run.run_id, "detect", None)
                .map_err(|e| e.to_string())
        },
        |task| {
            let output = if task.asset_path.ends_with("a.jpg") {
                Err("decode failed".into())
            } else {
                Ok(FaceOutput {
                    detections: Vec::new(),
                    features: Ok(Vec::new()),
                })
            };
            InferredAsset { task, output }
        },
        |asset| save(&people, asset, "encode", &work),
        || work.is_cancelled(),
    )
    .unwrap();
    let result = people.get_person_analysis(&run.run_id).unwrap();
    assert_eq!(result.state, PersonAnalysisState::Failed);
    assert_eq!((result.completed_tasks, result.failed_tasks), (2, 2));
}

#[test]
fn source_change_and_encoder_failure_are_recorded_without_false_success() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().canonicalize().unwrap();
    let people = testing::in_memory();
    let run = enroll(&people, &folder, "begin");
    let operations = PersonOperations::default();
    let work = operation(&operations);
    let first = people
        .claim_person_analysis_stage(&run.run_id, "detect", None)
        .unwrap()
        .unwrap();
    assert!(
        people
            .claim_person_analysis_stage(&run.run_id, "encode", Some(&first.asset_path))
            .unwrap()
            .is_none()
    );
    std::fs::write(&first.asset_path, b"changed source bytes").unwrap();
    save(
        &people,
        InferredAsset {
            task: first,
            output: Ok(FaceOutput {
                detections: Vec::new(),
                features: Ok(Vec::new()),
            }),
        },
        "encode",
        &work,
    )
    .unwrap();
    let second = people
        .claim_person_analysis_stage(&run.run_id, "detect", None)
        .unwrap()
        .unwrap();
    save(
        &people,
        InferredAsset {
            task: second,
            output: Ok(FaceOutput {
                detections: Vec::new(),
                features: Err("encoder failed".into()),
            }),
        },
        "encode",
        &work,
    )
    .unwrap();
    let result = people.get_person_analysis(&run.run_id).unwrap();
    assert_eq!((result.completed_tasks, result.failed_tasks), (1, 3));
    assert_eq!(result.state, PersonAnalysisState::Failed);
}

#[test]
fn superseded_and_cancelled_results_never_publish() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().canonicalize().unwrap();
    let people = testing::in_memory();
    let run = enroll(&people, &folder, "begin");
    let operations = PersonOperations::default();
    let work = operation(&operations);
    let task = people
        .claim_person_analysis_stage(&run.run_id, "detect", None)
        .unwrap()
        .unwrap();
    let replacement = enroll(&people, &folder, "replacement");
    save(
        &people,
        InferredAsset {
            task,
            output: Ok(FaceOutput {
                detections: Vec::new(),
                features: Ok(Vec::new()),
            }),
        },
        "encode",
        &work,
    )
    .unwrap();
    assert_eq!(
        people
            .get_person_analysis(&run.run_id)
            .unwrap()
            .completed_tasks,
        0
    );
    let task = people
        .claim_person_analysis_stage(&replacement.run_id, "detect", None)
        .unwrap()
        .unwrap();
    operations.cancel("test").unwrap();
    save(
        &people,
        InferredAsset {
            task,
            output: Ok(FaceOutput {
                detections: Vec::new(),
                features: Ok(Vec::new()),
            }),
        },
        "encode",
        &work,
    )
    .unwrap();
    assert_eq!(
        people
            .get_person_analysis(&replacement.run_id)
            .unwrap()
            .completed_tasks,
        0
    );
}
