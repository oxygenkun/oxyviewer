use super::*;

fn face(path: &str, angle: f32) -> Face {
    Face {
        member: PersonClusterMember {
            asset_path: path.into(),
            instance_id: path.into(),
            source_revision: "a".repeat(64),
            summary_revision: "1:1".into(),
            face_box: [0.1, 0.1, 0.2, 0.2],
        },
        values: Some(vec![angle.cos(), angle.sin()]),
        quality: 1.0,
    }
}

#[test]
fn complete_link_does_not_bridge_different_people_or_merge_same_photo() {
    let (groups, single) = group(
        vec![face("a", 0.0), face("b", 0.7), face("c", 1.4)],
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].members.len(), 2);
    assert_eq!(single.len(), 1);
    let (groups, single) = group(
        vec![face("a", 0.0), face("a", 0.0)],
        &CancellationToken::default(),
    )
    .unwrap();
    assert!(groups.is_empty());
    assert_eq!(single.len(), 2);
}

#[test]
fn missing_features_remain_singletons_and_cancellation_stops_work() {
    let mut missing = face("d", 0.0);
    missing.values = None;
    let (groups, single) = group(
        vec![face("a", -0.7), face("b", 0.7), face("c", 0.0), missing],
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(single.len(), 2);
    let token = CancellationToken::default();
    token.cancel();
    assert!(group(vec![face("a", 0.0)], &token).is_err());
}

pub(crate) fn analyzed_fixture(people: &People, folder: &Path) {
    use crate::people::types::{
        BeginPersonAnalysis, DetectedPersonInstance, FeatureModality, PersonAnalysisTaskInput,
    };
    let manifest = catalog::manifest();
    let pipeline = crate::people::pipeline_fingerprint(&manifest).unwrap();
    let (detector, encoder, space) = parameters().unwrap();
    let run = people
        .begin_person_analysis(&BeginPersonAnalysis {
            folder_path: folder.into(),
            pipeline_id: manifest.pipeline_id,
            pipeline_fingerprint: pipeline.clone(),
            request_id: format!("fixture:{}", folder.display()),
        })
        .unwrap();
    let mut tasks = Vec::new();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        let path = folder.join(name);
        std::fs::write(&path, b"source").unwrap();
        for (index, (id, producer)) in
            [(catalog::DETECTOR, &detector), (catalog::ENCODER, &encoder)]
                .into_iter()
                .enumerate()
        {
            tasks.push(PersonAnalysisTaskInput {
                asset_path: path.clone(),
                source_revision: crate::files::observe_file(&path).unwrap().revision_id(),
                stage_id: id.into(),
                stage_fingerprint: producer.clone(),
                stage_order: index as u32,
            });
        }
    }
    people
        .enqueue_person_analysis_tasks(&run.run_id, &tasks)
        .unwrap();
    people.seal_person_analysis_tasks(&run.run_id).unwrap();
    while people.person_analysis_is_current(&run.run_id).unwrap() {
        let Some(task) = people.claim_person_analysis_task(&run.run_id).unwrap() else {
            break;
        };
        if task.stage_id == catalog::DETECTOR {
            let detections = if task.asset_path.ends_with("c.jpg") {
                vec![]
            } else {
                vec![DetectedPersonInstance {
                    instance_id: "c".repeat(64),
                    face_box: Some([0.1, 0.1, 0.2, 0.2]),
                    face_landmarks: None,
                    body_box: None,
                    face_score: Some(0.9),
                    body_score: None,
                    association_score: None,
                }]
            };
            people
                .complete_person_analysis_task_with_detections(&task, &[], Some(&detections))
                .unwrap();
        } else {
            let mut vector = vec![0.0; 512];
            vector[0] = 1.0;
            let features = if task.asset_path.ends_with("c.jpg") {
                vec![]
            } else {
                vec![crate::people::features::PersonFeature {
                    folder_path: folder.into(),
                    asset_path: task.asset_path.clone(),
                    instance_id: "c".repeat(64),
                    source_revision: task.source_revision.clone(),
                    feature_space_id: space.clone(),
                    modality: FeatureModality::Face,
                    producer_fingerprint: encoder.clone(),
                    pipeline_fingerprint: pipeline.clone(),
                    values: vector,
                }]
            };
            people
                .complete_person_analysis_task(&task, &features)
                .unwrap();
        }
    }
}

#[test]
fn groups_survive_reopen_and_cache_clear_preserves_global_names() {
    use std::sync::Arc;
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().canonicalize().unwrap();
    let db = folder.join("test.sqlite");
    let store = Arc::new(crate::storage::Store::open(&db).unwrap());
    let people = People::new(Arc::clone(&store));
    analyzed_fixture(&people, &folder);
    let snapshot = people
        .cluster_folder(&folder, &CancellationToken::default())
        .unwrap();
    assert_eq!(snapshot.clusters.len(), 1);
    assert_eq!(snapshot.no_face_count, 1);
    assert!(people.global_people().unwrap().is_empty());
    assert!(
        crate::people::repo::global::tuples(&store.read(), None)
            .unwrap()
            .is_empty()
    );
    let person = people
        .save_global_person(&crate::people::types::SaveGlobalPerson {
            id: None,
            display_name: "Ada".into(),
            expected_revision: 0,
            request_id: "person".into(),
        })
        .unwrap();
    let restarted = People::new(Arc::new(crate::storage::Store::open(&db).unwrap()));
    assert_eq!(restarted.person_clusters(&folder).unwrap(), Some(snapshot));
    store.clear_rebuildable_cache().unwrap();
    assert!(people.person_clusters(&folder).unwrap().is_none());
    assert_eq!(people.global_people().unwrap()[0], person);
}

#[test]
fn stale_source_cancellation_and_new_run_cannot_publish_old_groups() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().canonicalize().unwrap();
    let people = crate::people::testing::in_memory();
    analyzed_fixture(&people, &folder);
    let token = CancellationToken::default();
    let snapshot = people.cluster_folder(&folder, &token).unwrap();
    token.cancel();
    assert!(people.cluster_folder(&folder, &token).is_err());
    assert_eq!(people.person_clusters(&folder).unwrap(), Some(snapshot));
    std::fs::write(folder.join("a.jpg"), b"changed source").unwrap();
    let refreshed = people
        .cluster_folder(&folder, &CancellationToken::default())
        .unwrap();
    assert!(refreshed.clusters.is_empty());
    assert_eq!(refreshed.ungrouped.len(), 1);
    assert_eq!(refreshed.ungrouped[0].asset_path, folder.join("b.jpg"));
    assert_eq!(refreshed.unavailable_count, 1);
    assert!(people.global_people().unwrap().is_empty());
    people
        .begin_person_analysis(&crate::people::types::BeginPersonAnalysis {
            folder_path: folder.clone(),
            pipeline_id: "new".into(),
            pipeline_fingerprint: "b".repeat(64),
            request_id: "new-run".into(),
        })
        .unwrap();
    assert!(people.person_clusters(&folder).unwrap().is_none());
}

#[test]
#[ignore = "requires an explicit disposable database snapshot and real photo folder"]
fn real_folder_cluster_fixture() {
    use std::sync::Arc;
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var("OXY_CLUSTER_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let store = Arc::new(
        crate::storage::Store::open(Path::new(fixture["database"].as_str().unwrap())).unwrap(),
    );
    let people = People::new(store);
    let started = std::time::Instant::now();
    let snapshot = people
        .cluster_folder(
            Path::new(fixture["folder"].as_str().unwrap()),
            &CancellationToken::default(),
        )
        .unwrap();
    println!(
        "clusters={} members={} ungrouped={} no_face={} unavailable={} elapsed_ms={}",
        snapshot.clusters.len(),
        snapshot
            .clusters
            .iter()
            .map(|c| c.members.len())
            .sum::<usize>(),
        snapshot.ungrouped.len(),
        snapshot.no_face_count,
        snapshot.unavailable_count,
        started.elapsed().as_millis()
    );
    std::fs::write(
        fixture["output"].as_str().unwrap(),
        serde_json::to_vec_pretty(&snapshot).unwrap(),
    )
    .unwrap();
}
