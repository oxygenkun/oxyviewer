//! Rebuildable model detections. Manual boxes and review decisions live in
//! separate tables and are never written through this cache.

use crate::{Library, LibraryError};
use oxy_domain::DetectedPersonInstance;
use rusqlite::{Transaction, params};
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

pub(crate) fn replace_stage_detections(
    transaction: &Transaction<'_>,
    context: &DetectionStageContext<'_>,
    detections: &[DetectedPersonInstance],
) -> Result<(), LibraryError> {
    if detections.len() > 1000 || detections.iter().any(|item| !valid_detection(item)) || {
        let mut ids = std::collections::HashSet::new();
        detections
            .iter()
            .any(|item| !ids.insert(item.instance_id.as_str()))
    } {
        return Err(LibraryError::InvalidPersonDetection);
    }
    transaction.execute(
        "DELETE FROM person_instances_cache WHERE folder_path=?1 AND asset_path=?2
           AND producer_fingerprint=?3",
        params![
            context.folder.to_string_lossy(),
            context.asset.to_string_lossy(),
            context.producer_fingerprint
        ],
    )?;
    for item in detections {
        transaction.execute(
            "INSERT INTO person_instances_cache(folder_path,asset_path,instance_id,source_revision,
               producer_fingerprint,pipeline_fingerprint,run_id,face_box,face_landmarks,body_box,
               face_score,body_score,association_score)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                context.folder.to_string_lossy(),
                context.asset.to_string_lossy(),
                item.instance_id,
                context.source_revision,
                context.producer_fingerprint,
                context.pipeline_fingerprint,
                context.run_id,
                item.face_box
                    .map(|value| serde_json::to_string(&value))
                    .transpose()?,
                item.face_landmarks
                    .map(|value| serde_json::to_string(&value))
                    .transpose()?,
                item.body_box
                    .map(|value| serde_json::to_string(&value))
                    .transpose()?,
                item.face_score,
                item.body_score,
                item.association_score
            ],
        )?;
    }
    Ok(())
}

impl Library {
    /// Only returns detections for the caller's current source and producer.
    pub fn list_person_detections(
        &self,
        folder: &Path,
        asset: &Path,
        source_revision: &str,
        producer_fingerprint: &str,
    ) -> Result<Vec<DetectedPersonInstance>, LibraryError> {
        if asset.parent() != Some(folder)
            || source_revision.is_empty()
            || producer_fingerprint.len() != 64
        {
            return Err(LibraryError::InvalidPersonDetection);
        }
        let connection = self.read_connection();
        let mut query = connection.prepare(
            "SELECT instance_id,face_box,face_landmarks,body_box,face_score,body_score,association_score
             FROM person_instances_cache WHERE folder_path=?1 AND asset_path=?2
               AND source_revision=?3 AND producer_fingerprint=?4 ORDER BY instance_id",
        )?;
        let rows = query.query_map(
            params![
                folder.to_string_lossy(),
                asset.to_string_lossy(),
                source_revision,
                producer_fingerprint
            ],
            |row| {
                let face: Option<String> = row.get("face_box")?;
                let landmarks: Option<String> = row.get("face_landmarks")?;
                let body: Option<String> = row.get("body_box")?;
                let parse = |value: Option<String>| {
                    value
                        .map(|value| serde_json::from_str::<[f64; 4]>(&value))
                        .transpose()
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })
                };
                Ok(DetectedPersonInstance {
                    instance_id: row.get("instance_id")?,
                    face_box: parse(face)?,
                    face_landmarks: landmarks
                        .map(|value| serde_json::from_str::<[[f32; 2]; 5]>(&value))
                        .transpose()
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                    body_box: parse(body)?,
                    face_score: row.get("face_score")?,
                    body_score: row.get("body_score")?,
                    association_score: row.get("association_score")?,
                })
            },
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_domain::{BeginPersonAnalysis, PersonAnalysisTaskInput};

    #[test]
    fn detection_commit_is_atomic_source_scoped_and_replaces_stage_set() {
        let temporary = tempfile::tempdir().unwrap();
        let folder = temporary.path().canonicalize().unwrap();
        let asset = folder.join("a.jpg");
        std::fs::write(&asset, b"image bytes").unwrap();
        let library = Library::in_memory().unwrap();
        let source_revision = oxy_fs::observe_file(&asset).unwrap().revision_id();
        let stage_fingerprint = "b".repeat(64);
        let pipeline_fingerprint = "a".repeat(64);
        let start = |request_id: &str| {
            let run = library
                .begin_person_analysis(&BeginPersonAnalysis {
                    folder_path: folder.clone(),
                    pipeline_id: "test".into(),
                    pipeline_fingerprint: pipeline_fingerprint.clone(),
                    request_id: request_id.into(),
                })
                .unwrap();
            library
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
            library
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
            library.complete_person_analysis_task_with_detections(&first, &[], Some(&[invalid])),
            Err(LibraryError::InvalidPersonDetection)
        ));
        let mut invalid_landmarks = detection.clone();
        invalid_landmarks.face_landmarks = Some([[f32::INFINITY, 0.0]; 5]);
        assert!(matches!(
            library.complete_person_analysis_task_with_detections(
                &first,
                &[],
                Some(&[invalid_landmarks])
            ),
            Err(LibraryError::InvalidPersonDetection)
        ));
        assert!(
            library
                .list_person_detections(&folder, &asset, &source_revision, &stage_fingerprint)
                .unwrap()
                .is_empty()
        );
        library
            .complete_person_analysis_task_with_detections(
                &first,
                &[],
                Some(std::slice::from_ref(&detection)),
            )
            .unwrap();
        assert_eq!(
            library
                .list_person_detections(&folder, &asset, &source_revision, &stage_fingerprint)
                .unwrap(),
            vec![detection]
        );
        assert!(
            library
                .list_person_detections(&folder, &asset, "changed-source", &stage_fingerprint)
                .unwrap()
                .is_empty()
        );
        let second = start("detect-second");
        library
            .complete_person_analysis_task_with_detections(&second, &[], Some(&[]))
            .unwrap();
        assert!(
            library
                .list_person_detections(&folder, &asset, &source_revision, &stage_fingerprint)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn incompatible_detection_cache_rebuild_preserves_manual_instance() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let library = Library::open(&database).unwrap();
        let folder = std::path::Path::new("/photos");
        let asset = folder.join("a.jpg");
        let manual = library
            .create_person_instance(&oxy_domain::CreatePersonInstance {
                folder_path: folder.to_path_buf(),
                asset_path: asset.clone(),
                source_revision: "10:20".into(),
                face_box: Some([0.1, 0.1, 0.2, 0.2]),
                body_box: None,
                request_id: "manual".into(),
            })
            .unwrap();
        library
            .write()
            .execute(
                "INSERT INTO person_instances_cache(folder_path,asset_path,instance_id,
               source_revision,producer_fingerprint,pipeline_fingerprint,run_id,face_box)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    "/photos",
                    "/photos/a.jpg",
                    "c".repeat(64),
                    "source",
                    "b".repeat(64),
                    "a".repeat(64),
                    "run",
                    "[0.1,0.1,0.2,0.2]"
                ],
            )
            .unwrap();
        library
            .write()
            .execute(
                "UPDATE person_detection_cache_meta SET schema_version=1",
                [],
            )
            .unwrap();
        drop(library);
        let reopened = Library::open(&database).unwrap();
        assert_eq!(
            reopened.list_person_instances(folder, &asset).unwrap()[0].id,
            manual.id
        );
        assert!(
            reopened
                .list_person_detections(folder, &asset, "source", &"b".repeat(64))
                .unwrap()
                .is_empty()
        );
    }
}
