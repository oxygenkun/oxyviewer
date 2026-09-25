//! Rebuildable model detections. Manual boxes and review decisions live in
//! separate tables and are never written through this cache.
//!
//! The tables and the statements live in `oxy_store`; what stays here is what a
//! detection *means* — that a box is inside the image, that a stage replaces
//! its own set and nothing else's, and that a detection never answers a review
//! decision.

use crate::{People, PeopleError};
use oxy_domain::DetectedPersonInstance;
use oxy_store::{Transaction, repo};
use std::path::Path;

fn valid_box(value: Option<[f64; 4]>) -> bool {
    value.is_none_or(|[x, y, width, height]| {
        [x, y, width, height].iter().all(|part| part.is_finite())
            && x >= 0.0
            && y >= 0.0
            && width > 0.0
            && height > 0.0
            && x + width <= 1.0
            && y + height <= 1.0
    })
}

fn valid_score(value: Option<f32>) -> bool {
    value.is_none_or(|value| value.is_finite() && (0.0..=1.0).contains(&value))
}

fn valid_detection(item: &DetectedPersonInstance) -> bool {
    item.instance_id.len() == 64
        && item
            .instance_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        && (item.face_box.is_some() || item.body_box.is_some())
        && valid_box(item.face_box)
        && (item.face_box.is_some() || item.face_landmarks.is_none())
        && item
            .face_landmarks
            .is_none_or(|points| points.iter().flatten().all(|value| value.is_finite()))
        && valid_box(item.body_box)
        && valid_score(item.face_score)
        && valid_score(item.body_score)
        && valid_score(item.association_score)
        && (item.face_box.is_some() || item.face_score.is_none())
        && (item.body_box.is_some() || item.body_score.is_none())
        && (item.face_box.is_some() && item.body_box.is_some() || item.association_score.is_none())
}

pub(crate) struct DetectionStageContext<'a> {
    pub folder: &'a Path,
    pub asset: &'a Path,
    pub source_revision: &'a str,
    pub producer_fingerprint: &'a str,
    pub pipeline_fingerprint: &'a str,
    pub run_id: &'a str,
}

/// Replaces one stage's detections for one asset, inside the caller's
/// transaction.
///
/// Whole-set replacement is the rule: a stage that found nothing supplies an
/// empty slice, which is not the same as supplying no slice at all.
pub(crate) fn replace_stage_detections(
    transaction: &Transaction<'_>,
    context: &DetectionStageContext<'_>,
    detections: &[DetectedPersonInstance],
) -> Result<(), PeopleError> {
    if detections.len() > 1000 || detections.iter().any(|item| !valid_detection(item)) || {
        let mut ids = std::collections::HashSet::new();
        detections
            .iter()
            .any(|item| !ids.insert(item.instance_id.as_str()))
    } {
        return Err(PeopleError::InvalidPersonDetection);
    }
    let folder = context.folder.to_string_lossy();
    let asset = context.asset.to_string_lossy();
    repo::person_cache::clear_stage_detections(
        transaction,
        &folder,
        &asset,
        context.producer_fingerprint,
    )?;
    for item in detections {
        repo::person_cache::insert_detection(
            transaction,
            &repo::person_cache::NewDetection {
                folder_path: &folder,
                asset_path: &asset,
                instance_id: &item.instance_id,
                source_revision: context.source_revision,
                producer_fingerprint: context.producer_fingerprint,
                pipeline_fingerprint: context.pipeline_fingerprint,
                run_id: context.run_id,
                face_box: item.face_box,
                face_landmarks: item.face_landmarks,
                body_box: item.body_box,
                face_score: item.face_score,
                body_score: item.body_score,
                association_score: item.association_score,
            },
        )?;
    }
    Ok(())
}

impl People {
    /// Only returns detections for the caller's current source and producer.
    pub fn list_person_detections(
        &self,
        folder: &Path,
        asset: &Path,
        source_revision: &str,
        producer_fingerprint: &str,
    ) -> Result<Vec<DetectedPersonInstance>, PeopleError> {
        if asset.parent() != Some(folder)
            || source_revision.is_empty()
            || producer_fingerprint.len() != 64
        {
            return Err(PeopleError::InvalidPersonDetection);
        }
        Ok(repo::person_cache::list_detections(
            &self.store.read(),
            &folder.to_string_lossy(),
            &asset.to_string_lossy(),
            source_revision,
            producer_fingerprint,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use oxy_domain::{BeginPersonAnalysis, PersonAnalysisTaskInput};

    #[test]
    fn detection_commit_is_atomic_source_scoped_and_replaces_stage_set() {
        let temporary = tempfile::tempdir().unwrap();
        let folder = temporary.path().canonicalize().unwrap();
        let asset = folder.join("a.jpg");
        std::fs::write(&asset, b"image bytes").unwrap();
        let people = testing::in_memory();
        let source_revision = oxy_fs::observe_file(&asset).unwrap().revision_id();
        let stage_fingerprint = "b".repeat(64);
        let pipeline_fingerprint = "a".repeat(64);
        let start = |request_id: &str| {
            let run = people
                .begin_person_analysis(&BeginPersonAnalysis {
                    folder_path: folder.clone(),
                    pipeline_id: "test".into(),
                    pipeline_fingerprint: pipeline_fingerprint.clone(),
                    request_id: request_id.into(),
                })
                .unwrap();
            people
                .enqueue_person_analysis_tasks(
                    &run.run_id,
                    &[PersonAnalysisTaskInput {
                        asset_path: asset.clone(),
                        source_revision: source_revision.clone(),
                        stage_id: "detect".into(),
                        stage_fingerprint: stage_fingerprint.clone(),
                        stage_order: 0,
                    }],
                )
                .unwrap();
            people
                .claim_person_analysis_task(&run.run_id)
                .unwrap()
                .unwrap()
        };
        let first = start("detect-first");
        let detection = DetectedPersonInstance {
            instance_id: "c".repeat(64),
            face_box: Some([0.1, 0.1, 0.2, 0.2]),
            face_landmarks: Some([[0.12, 0.36]; 5]),
            body_box: None,
            face_score: Some(0.9),
            body_score: None,
            association_score: None,
        };
        let mut invalid = detection.clone();
        invalid.face_score = Some(f32::NAN);
        assert!(matches!(
            people.complete_person_analysis_task_with_detections(&first, &[], Some(&[invalid])),
            Err(PeopleError::InvalidPersonDetection)
        ));
        let mut invalid_landmarks = detection.clone();
        invalid_landmarks.face_landmarks = Some([[f32::INFINITY, 0.0]; 5]);
        assert!(matches!(
            people.complete_person_analysis_task_with_detections(
                &first,
                &[],
                Some(&[invalid_landmarks])
            ),
            Err(PeopleError::InvalidPersonDetection)
        ));
        assert!(
            people
                .list_person_detections(&folder, &asset, &source_revision, &stage_fingerprint)
                .unwrap()
                .is_empty()
        );
        people
            .complete_person_analysis_task_with_detections(
                &first,
                &[],
                Some(std::slice::from_ref(&detection)),
            )
            .unwrap();
        assert_eq!(
            people
                .list_person_detections(&folder, &asset, &source_revision, &stage_fingerprint)
                .unwrap(),
            vec![detection]
        );
        assert!(
            people
                .list_person_detections(&folder, &asset, "changed-source", &stage_fingerprint)
                .unwrap()
                .is_empty()
        );
        let second = start("detect-second");
        people
            .complete_person_analysis_task_with_detections(&second, &[], Some(&[]))
            .unwrap();
        assert!(
            people
                .list_person_detections(&folder, &asset, &source_revision, &stage_fingerprint)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn incompatible_detection_cache_rebuild_preserves_manual_instance() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let people = testing::open(&database);
        let folder = std::path::Path::new("/photos");
        let asset = folder.join("a.jpg");
        let manual = people
            .create_person_instance(&oxy_domain::CreatePersonInstance {
                folder_path: folder.to_path_buf(),
                asset_path: asset.clone(),
                source_revision: "10:20".into(),
                face_box: Some([0.1, 0.1, 0.2, 0.2]),
                body_box: None,
                request_id: "manual".into(),
            })
            .unwrap();
        let producer = "b".repeat(64);
        let pipeline = "a".repeat(64);
        {
            let connection = people.store.write();
            repo::person_cache::insert_detection(
                &connection,
                &repo::person_cache::NewDetection {
                    folder_path: "/photos",
                    asset_path: "/photos/a.jpg",
                    instance_id: &"c".repeat(64),
                    source_revision: "source",
                    producer_fingerprint: &producer,
                    pipeline_fingerprint: &pipeline,
                    run_id: "run",
                    face_box: Some([0.1, 0.1, 0.2, 0.2]),
                    face_landmarks: None,
                    body_box: None,
                    face_score: None,
                    body_score: None,
                    association_score: None,
                },
            )
            .unwrap();
            // Pretend an earlier build wrote this cache. The marker belongs to
            // `oxy_store::schema`, so only a direct write can stage that state;
            // production code in this crate carries no SQL.
            connection
                .execute(
                    "UPDATE person_detection_cache_meta SET schema_version=1",
                    [],
                )
                .unwrap();
        }
        drop(people);
        let reopened = testing::open(&database);
        assert_eq!(
            reopened.list_person_instances(folder, &asset).unwrap()[0].id,
            manual.id
        );
        assert!(
            reopened
                .list_person_detections(folder, &asset, "source", &producer)
                .unwrap()
                .is_empty()
        );
    }
}
