//! Explicit, cancellable enrollment of one folder's current asset snapshot.
//! Inference and storage are supplied by callers; this module never starts
//! work as part of the interactive folder-open path.

use crate::{PipelineError, pipeline_fingerprint, require_installable, stage_fingerprint};
use oxy_domain::{
    PersonAnalysisRun, PersonAnalysisState, PersonAnalysisTaskInput, PersonStageKind,
    PipelineManifest,
};
use oxy_fs::{FsCatalog, FsError, observe_file, scan_assets_with_progress_and_cancel};
use std::{collections::HashMap, error::Error, path::Path};
use thiserror::Error;

const TASK_BATCH_SIZE: usize = 256;

pub trait AnalysisTaskSink {
    type Error: Error + Send + Sync + 'static;

    fn enqueue(
        &mut self,
        run_id: &str,
        tasks: &[PersonAnalysisTaskInput],
    ) -> Result<usize, Self::Error>;
}

#[derive(Debug, Error)]
pub enum AnalysisEnrollmentError<E: Error + 'static> {
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    #[error(transparent)]
    Filesystem(#[from] FsError),
    #[error("analysis run does not match the folder or pipeline")]
    RunMismatch,
    #[error("analysis was cancelled during snapshot enrollment")]
    Cancelled,
    #[error("asset moved outside the selected folder or appeared twice")]
    InvalidAsset,
    #[error("analysis task store failed: {0}")]
    Store(E),
}

fn is_asset_stage(kind: PersonStageKind) -> bool {
    !matches!(
        kind,
        PersonStageKind::Clusterer | PersonStageKind::Retriever
    )
}

/// Return a stable topological order for per-asset stages. Folder-wide stages
/// are scheduled separately after per-asset evidence exists.
pub fn asset_stage_order(manifest: &PipelineManifest) -> Result<Vec<String>, PipelineError> {
    require_installable(manifest)?;
    let stages: HashMap<_, _> = manifest
        .stages
        .iter()
        .map(|stage| (stage.stage_id.as_str(), stage))
        .collect();
    let mut ids: Vec<_> = manifest
        .stages
        .iter()
        .filter(|stage| is_asset_stage(stage.kind))
        .map(|stage| stage.stage_id.as_str())
        .collect();
    ids.sort_unstable();
    let mut ordered = Vec::with_capacity(ids.len());
    let mut visited = std::collections::HashSet::new();
    fn visit<'a>(
        id: &'a str,
        stages: &HashMap<&'a str, &'a oxy_domain::PersonStageManifest>,
        visited: &mut std::collections::HashSet<&'a str>,
        ordered: &mut Vec<String>,
    ) -> Result<(), PipelineError> {
        if !visited.insert(id) {
            return Ok(());
        }
        let stage = stages.get(id).ok_or(PipelineError::InvalidDependency)?;
        let mut dependencies: Vec<_> = stage.depends_on.iter().map(String::as_str).collect();
        dependencies.sort_unstable();
        for dependency in dependencies {
            let parent = stages
                .get(dependency)
                .ok_or(PipelineError::InvalidDependency)?;
            if !is_asset_stage(parent.kind) {
                return Err(PipelineError::InvalidContract);
            }
            visit(dependency, stages, visited, ordered)?;
        }
        ordered.push(id.to_owned());
        Ok(())
    }
    for id in ids {
        visit(id, &stages, &mut visited, &mut ordered)?;
    }
    Ok(ordered)
}

/// Enrolls one stable `oxy-fs` snapshot in small transactions. The snapshot
/// itself is non-recursive. Source versions are observed from open file handles;
/// the result-commit path observes each file again after inference.
pub fn enroll_folder_snapshot<S: AnalysisTaskSink>(
    catalog: &FsCatalog,
    session_id: &str,
    run: &PersonAnalysisRun,
    manifest: &PipelineManifest,
    sink: &mut S,
    cancelled: impl Fn() -> bool,
) -> Result<usize, AnalysisEnrollmentError<S::Error>> {
    let stages = asset_stage_order(manifest)?;
    let fingerprint = pipeline_fingerprint(manifest)?;
    if !matches!(
        run.state,
        PersonAnalysisState::Queued | PersonAnalysisState::Running
    ) || run.pipeline_id != manifest.pipeline_id
        || run.pipeline_fingerprint != fingerprint
        || run.folder_path != catalog.session_directory(session_id, Some(&run.folder_path))?
    {
        return Err(AnalysisEnrollmentError::RunMismatch);
    }
    let folder = run.folder_path.as_path();
    // An explicit analysis run must include files added since the browsing
    // cache was filled. Keep this fresh scan separate from the open path.
    let mut assets = match scan_assets_with_progress_and_cancel(folder, |_| {}, &cancelled) {
        Err(FsError::Cancelled) => return Err(AnalysisEnrollmentError::Cancelled),
        result => result?,
    };
    assets.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    let fingerprints = stages
        .iter()
        .map(|stage_id| Ok((stage_id.clone(), stage_fingerprint(manifest, stage_id)?)))
        .collect::<Result<Vec<_>, PipelineError>>()?;
    let mut batch = Vec::with_capacity(TASK_BATCH_SIZE);
    let mut enrolled = 0usize;
    let mut previous: Option<&Path> = None;
    for asset in &assets {
        if cancelled() {
            return Err(AnalysisEnrollmentError::Cancelled);
        }
        if asset.path.parent() != Some(folder) || previous == Some(asset.path.as_path()) {
            return Err(AnalysisEnrollmentError::InvalidAsset);
        }
        previous = Some(asset.path.as_path());
        let observation = observe_file(&asset.path).map_err(FsError::from)?;
        if observation.canonical_path != asset.path {
            return Err(AnalysisEnrollmentError::InvalidAsset);
        }
        let revision = observation.revision_id();
        for (order, (stage_id, stage_fingerprint)) in fingerprints.iter().enumerate() {
            batch.push(PersonAnalysisTaskInput {
                asset_path: asset.path.clone(),
                source_revision: revision.clone(),
                stage_id: stage_id.clone(),
                stage_fingerprint: stage_fingerprint.clone(),
                stage_order: order as u32,
            });
            if batch.len() == TASK_BATCH_SIZE {
                if cancelled() {
                    return Err(AnalysisEnrollmentError::Cancelled);
                }
                enrolled += sink
                    .enqueue(&run.run_id, &batch)
                    .map_err(AnalysisEnrollmentError::Store)?;
                batch.clear();
            }
        }
    }
    if !batch.is_empty() {
        if cancelled() {
            return Err(AnalysisEnrollmentError::Cancelled);
        }
        enrolled += sink
            .enqueue(&run.run_id, &batch)
            .map_err(AnalysisEnrollmentError::Store)?;
    }
    if cancelled() {
        return Err(AnalysisEnrollmentError::Cancelled);
    }
    // The full run also needs folder-level clustering and retrieval tasks.
    // Their scheduler, not per-asset enrollment, owns enumeration sealing.
    Ok(enrolled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_domain::{
        BeginPersonAnalysis, PersonDistanceMetric, PersonModelArtifact, PersonModelArtifactFormat,
        PersonModelLicenseStatus, PersonStageManifest,
    };
    use crate::testing;
    use crate::{People, PeopleError};
    use std::{fs, path::PathBuf};

    struct PeopleSink<'a> {
        people: &'a People,
        batch_sizes: Vec<usize>,
    }

    impl AnalysisTaskSink for PeopleSink<'_> {
        type Error = PeopleError;

        fn enqueue(
            &mut self,
            run_id: &str,
            tasks: &[PersonAnalysisTaskInput],
        ) -> Result<usize, Self::Error> {
            self.batch_sizes.push(tasks.len());
            self.people.enqueue_person_analysis_tasks(run_id, tasks)
        }
    }

    fn manifest() -> PipelineManifest {
        let mut stages = Vec::new();
        for (id, kind, dependencies) in [
            ("face", PersonStageKind::FaceDetector, vec![]),
            ("body", PersonStageKind::BodyDetector, vec![]),
            (
                "associate",
                PersonStageKind::Association,
                vec!["face", "body"],
            ),
            ("quality", PersonStageKind::QualityGate, vec!["associate"]),
            ("face-embed", PersonStageKind::FaceEncoder, vec!["quality"]),
            ("body-embed", PersonStageKind::BodyEncoder, vec!["quality"]),
            (
                "cluster",
                PersonStageKind::Clusterer,
                vec!["face-embed", "body-embed"],
            ),
            ("retrieve", PersonStageKind::Retriever, vec!["cluster"]),
        ] {
            let is_encoder = matches!(
                kind,
                PersonStageKind::FaceEncoder | PersonStageKind::BodyEncoder
            );
            let needs_model = matches!(
                kind,
                PersonStageKind::FaceDetector
                    | PersonStageKind::BodyDetector
                    | PersonStageKind::FaceEncoder
                    | PersonStageKind::BodyEncoder
            );
            stages.push(PersonStageManifest {
                stage_id: id.into(),
                kind,
                depends_on: dependencies.into_iter().map(str::to_owned).collect(),
                input_schema: "input-v1".into(),
                output_schema: "output-v1".into(),
                runtime_version: "test-runtime-v1".into(),
                preprocessing_version: "test-preprocess-v1".into(),
                postprocessing_version: "test-postprocess-v1".into(),
                feature_space_id: is_encoder.then(|| format!("{id}-space")),
                distance_metric: is_encoder.then_some(PersonDistanceMetric::Cosine),
                calibration_id: is_encoder.then(|| "test-calibration".into()),
                artifact: needs_model.then(|| PersonModelArtifact {
                    artifact_id: id.into(),
                    download_url: format!("https://example.org/{id}.onnx"),
                    format: PersonModelArtifactFormat::Onnx,
                    size_bytes: 100,
                    sha256: "a".repeat(64),
                    license_name: "Test".into(),
                    license_url: "https://example.org/license".into(),
                    license_status: PersonModelLicenseStatus::VerifiedForDistribution,
                }),
            });
        }
        stages.reverse();
        PipelineManifest {
            pipeline_id: "test-pipeline".into(),
            pipeline_version: "1".into(),
            stages,
        }
    }

    #[test]
    fn enrolls_current_layer_in_bounded_batches_with_real_revisions() {
        let temporary = tempfile::tempdir().unwrap();
        let folder = temporary.path().canonicalize().unwrap();
        for index in 0..43 {
            fs::write(folder.join(format!("{index:03}.jpg")), b"image").unwrap();
        }
        fs::write(folder.join("ignore.txt"), b"text").unwrap();
        fs::create_dir(folder.join("child")).unwrap();
        fs::write(folder.join("child").join("nested.jpg"), b"nested").unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(&folder).unwrap();
        assert_eq!(
            catalog
                .list_asset_candidates(&session.id, None)
                .unwrap()
                .len(),
            43
        );
        fs::write(folder.join("043.jpg"), b"new image").unwrap();
        let pipeline = manifest();
        let ordered = asset_stage_order(&pipeline).unwrap();
        assert_eq!(ordered.len(), 6);
        assert!(
            ordered.iter().position(|id| id == "associate").unwrap()
                > ordered.iter().position(|id| id == "face").unwrap()
        );
        assert!(
            ordered.iter().position(|id| id == "associate").unwrap()
                > ordered.iter().position(|id| id == "body").unwrap()
        );

        let people = testing::in_memory();
        let run = people
            .begin_person_analysis(&BeginPersonAnalysis {
                folder_path: folder.clone(),
                pipeline_id: pipeline.pipeline_id.clone(),
                pipeline_fingerprint: pipeline_fingerprint(&pipeline).unwrap(),
                request_id: "enroll-once".into(),
            })
            .unwrap();
        let mut sink = PeopleSink {
            people: &people,
            batch_sizes: Vec::new(),
        };
        let enrolled =
            enroll_folder_snapshot(&catalog, &session.id, &run, &pipeline, &mut sink, || false)
                .unwrap();
        assert_eq!(enrolled, 44 * 6);
        assert_eq!(sink.batch_sizes, vec![256, 8]);
        let current = people.get_person_analysis(&run.run_id).unwrap();
        assert!(!current.enumeration_complete);
        assert_eq!(current.total_tasks, enrolled as u64);
        let task = people
            .claim_person_analysis_task(&run.run_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            task.source_revision,
            observe_file(&task.asset_path).unwrap().revision_id()
        );
        assert_eq!(task.asset_path.parent(), Some(folder.as_path()));
        assert_ne!(task.asset_path, PathBuf::from("child/nested.jpg"));
        assert_eq!(
            people
                .complete_person_analysis_task(&task, &[])
                .unwrap()
                .completed_tasks,
            1
        );
    }

    #[test]
    fn cancellation_leaves_run_unsealed() {
        let temporary = tempfile::tempdir().unwrap();
        let folder = temporary.path().canonicalize().unwrap();
        fs::write(folder.join("image.jpg"), b"image").unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(&folder).unwrap();
        let pipeline = manifest();
        let people = testing::in_memory();
        let run = people
            .begin_person_analysis(&BeginPersonAnalysis {
                folder_path: folder,
                pipeline_id: pipeline.pipeline_id.clone(),
                pipeline_fingerprint: pipeline_fingerprint(&pipeline).unwrap(),
                request_id: "cancel-enroll".into(),
            })
            .unwrap();
        let mut sink = PeopleSink {
            people: &people,
            batch_sizes: Vec::new(),
        };
        assert!(matches!(
            enroll_folder_snapshot(&catalog, &session.id, &run, &pipeline, &mut sink, || true),
            Err(AnalysisEnrollmentError::Cancelled)
        ));
        let pending = people.get_person_analysis(&run.run_id).unwrap();
        assert!(!pending.enumeration_complete);
        assert_eq!(pending.total_tasks, 0);
    }
}
