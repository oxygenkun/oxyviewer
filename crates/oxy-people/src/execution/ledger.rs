//! The analysis run ledger: which run is the folder's head, which stages are
//! outstanding, and which commit is allowed to land.
//!
//! Every statement lives in `oxy_store::repo::person_cache`. What stays here is
//! the fence itself: that a run is only current while it is the head and
//! unfinished, that a claim token from a superseded worker may not publish, and
//! that a failure ends the stages that depended on it.

use crate::detections::{DetectionStageContext, replace_stage_detections};
use crate::features::{PersonFeature, validate_feature, write_feature};
use crate::{People, PeopleError};
use oxy_domain::{
    BeginPersonAnalysis, DetectedPersonInstance, PersonAnalysisRun, PersonAnalysisState,
    PersonAnalysisTask, PersonAnalysisTaskInput,
};
use oxy_store::{StoreError, Transaction, repo};

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The run a worker may still act on: it is the folder's head, and unfinished.
///
/// Both halves are one rule rather than two, because a superseded run is not
/// partially current — it is closed, and a late result must be refused.
fn current_run(
    transaction: &Transaction<'_>,
    run_id: &str,
) -> Result<PersonAnalysisRun, PeopleError> {
    let run = repo::person_cache::analysis_run(transaction, run_id)?
        .ok_or(PeopleError::MissingPersonAnalysis)?;
    let is_head = repo::person_cache::analysis_head_matches(
        transaction,
        &run.folder_path.to_string_lossy(),
        &run.run_id,
        run.generation,
    )?;
    if !is_head
        || !matches!(
            run.state,
            PersonAnalysisState::Queued | PersonAnalysisState::Running
        )
    {
        return Err(PeopleError::PersonAnalysisConflict);
    }
    Ok(run)
}

impl People {
    /// Starts a new folder generation. A replay of the same request returns its
    /// original run; a new request supersedes the previous active run.
    pub fn begin_person_analysis(
        &self,
        input: &BeginPersonAnalysis,
    ) -> Result<PersonAnalysisRun, PeopleError> {
        if input.folder_path.as_os_str().is_empty()
            || input.pipeline_id.trim().is_empty()
            || !valid_fingerprint(&input.pipeline_fingerprint)
            || input.request_id.trim().is_empty()
        {
            return Err(PeopleError::InvalidPersonAnalysis);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        if let Some((operation, run_id)) =
            repo::person_cache::analysis_request(&transaction, &input.request_id)?
        {
            if operation != "begin" {
                return Err(PeopleError::PersonAnalysisConflict);
            }
            let run = repo::person_cache::analysis_run(&transaction, &run_id)?
                .ok_or(PeopleError::MissingPersonAnalysis)?;
            if run.folder_path != input.folder_path
                || run.pipeline_id != input.pipeline_id
                || run.pipeline_fingerprint != input.pipeline_fingerprint
            {
                return Err(PeopleError::PersonAnalysisConflict);
            }
            return Ok(run);
        }
        let folder = input.folder_path.to_string_lossy();
        let previous = repo::person_cache::analysis_head(&transaction, &folder)?;
        let generation = previous
            .as_ref()
            .map_or(Some(1), |(value, _)| value.checked_add(1))
            .ok_or(PeopleError::PersonAnalysisConflict)?;
        if let Some((_, previous_id)) = previous {
            repo::person_cache::cancel_analysis_run(&transaction, &previous_id)?;
        }
        let run_id = repo::person_cache::new_cache_id(&transaction)?;
        repo::person_cache::insert_analysis_run(
            &transaction,
            &repo::person_cache::NewAnalysisRun {
                run_id: &run_id,
                folder_path: &folder,
                pipeline_id: &input.pipeline_id,
                pipeline_fingerprint: &input.pipeline_fingerprint,
                generation,
            },
        )?;
        repo::person_cache::insert_analysis_request(
            &transaction,
            &input.request_id,
            "begin",
            &run_id,
        )?;
        let run = repo::person_cache::analysis_run(&transaction, &run_id)?
            .ok_or(PeopleError::MissingPersonAnalysis)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(run)
    }

    pub fn get_person_analysis(&self, run_id: &str) -> Result<PersonAnalysisRun, PeopleError> {
        let connection = self.store.read();
        repo::person_cache::analysis_run(&connection, run_id)?
            .ok_or(PeopleError::MissingPersonAnalysis)
    }

    pub fn cancel_person_analysis(
        &self,
        run_id: &str,
        request_id: &str,
    ) -> Result<PersonAnalysisRun, PeopleError> {
        if run_id.is_empty() || request_id.trim().is_empty() {
            return Err(PeopleError::InvalidPersonAnalysis);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        if let Some((operation, prior_id)) =
            repo::person_cache::analysis_request(&transaction, request_id)?
        {
            if operation != "cancel" || prior_id != run_id {
                return Err(PeopleError::PersonAnalysisConflict);
            }
        } else {
            let changed = repo::person_cache::cancel_analysis_run(&transaction, run_id)?;
            if changed == 0 && !repo::person_cache::analysis_run_exists(&transaction, run_id)? {
                return Err(PeopleError::MissingPersonAnalysis);
            }
            repo::person_cache::insert_analysis_request(
                &transaction,
                request_id,
                "cancel",
                run_id,
            )?;
        }
        let run = repo::person_cache::analysis_run(&transaction, run_id)?
            .ok_or(PeopleError::MissingPersonAnalysis)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(run)
    }

    /// Registers at most one small batch from the current `oxy-fs` snapshot.
    /// Replaying identical tasks is safe; changed inputs require a new run.
    pub fn enqueue_person_analysis_tasks(
        &self,
        run_id: &str,
        tasks: &[PersonAnalysisTaskInput],
    ) -> Result<usize, PeopleError> {
        if tasks.is_empty() || tasks.len() > 256 {
            return Err(PeopleError::InvalidPersonAnalysis);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        let run = current_run(&transaction, run_id)?;
        if run.enumeration_complete {
            return Err(PeopleError::PersonAnalysisConflict);
        }
        let mut added = 0usize;
        for task in tasks {
            if task.asset_path.parent() != Some(run.folder_path.as_path())
                || task.source_revision.is_empty()
                || task.stage_id.trim().is_empty()
                || !valid_fingerprint(&task.stage_fingerprint)
            {
                return Err(PeopleError::InvalidPersonAnalysis);
            }
            let asset_path = task.asset_path.to_string_lossy();
            let existing = repo::person_cache::analysis_task_identity(
                &transaction,
                run_id,
                &asset_path,
                &task.stage_id,
            )?;
            if let Some((source, fingerprint, order)) = existing {
                if source != task.source_revision
                    || fingerprint != task.stage_fingerprint
                    || order != i64::from(task.stage_order)
                {
                    return Err(PeopleError::PersonAnalysisConflict);
                }
                continue;
            }
            if repo::person_cache::analysis_task_order_taken(
                &transaction,
                run_id,
                &asset_path,
                i64::from(task.stage_order),
            )? {
                return Err(PeopleError::PersonAnalysisConflict);
            }
            repo::person_cache::insert_analysis_task(
                &transaction,
                &repo::person_cache::NewAnalysisTask {
                    run_id,
                    asset_path: &asset_path,
                    source_revision: &task.source_revision,
                    stage_id: &task.stage_id,
                    stage_fingerprint: &task.stage_fingerprint,
                    stage_order: i64::from(task.stage_order),
                },
            )?;
            added += 1;
        }
        repo::person_cache::add_run_tasks(&transaction, run_id, added as i64)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(added)
    }

    pub fn seal_person_analysis_tasks(
        &self,
        run_id: &str,
    ) -> Result<PersonAnalysisRun, PeopleError> {
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        current_run(&transaction, run_id)?;
        repo::person_cache::seal_analysis_run(&transaction, run_id)?;
        let run = repo::person_cache::analysis_run(&transaction, run_id)?
            .ok_or(PeopleError::MissingPersonAnalysis)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(run)
    }

    /// Claims a task only after earlier stages for that asset have completed.
    pub fn claim_person_analysis_task(
        &self,
        run_id: &str,
    ) -> Result<Option<PersonAnalysisTask>, PeopleError> {
        self.claim_analysis_matching(run_id, None, None)
    }

    /// Pipeline preparation claims only detectors; publication claims the
    /// matching encoder after its detector transaction commits successfully.
    #[cfg(any(target_os = "windows", target_os = "macos", test))]
    pub(crate) fn claim_person_analysis_stage(
        &self,
        run_id: &str,
        stage_id: &str,
        asset_path: Option<&std::path::Path>,
    ) -> Result<Option<PersonAnalysisTask>, PeopleError> {
        let asset = asset_path.map(|path| path.to_string_lossy());
        self.claim_analysis_matching(run_id, Some(stage_id), asset.as_deref())
    }

    fn claim_analysis_matching(
        &self,
        run_id: &str,
        stage_id: Option<&str>,
        asset_path: Option<&str>,
    ) -> Result<Option<PersonAnalysisTask>, PeopleError> {
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        current_run(&transaction, run_id)?;
        let mut task =
            repo::person_cache::claimable_task(&transaction, run_id, stage_id, asset_path)?;
        if let Some(task) = &mut task {
            let claim_token = repo::person_cache::new_cache_id(&transaction)?;
            let changed = repo::person_cache::claim_analysis_task(
                &transaction,
                run_id,
                &task.asset_path.to_string_lossy(),
                &task.stage_id,
                &claim_token,
            )?;
            if changed != 1 {
                return Err(PeopleError::PersonAnalysisConflict);
            }
            task.claim_token = claim_token;
            repo::person_cache::start_analysis_run(&transaction, run_id)?;
        }
        transaction.commit().map_err(StoreError::from)?;
        Ok(task)
    }

    /// Requeues claims left by an interrupted worker. The old claim tokens are
    /// invalidated in the same transaction, so late workers cannot publish.
    /// Call this when starting or restarting the worker for a persisted run.
    pub fn recover_person_analysis_tasks(&self, run_id: &str) -> Result<usize, PeopleError> {
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        current_run(&transaction, run_id)?;
        let recovered = repo::person_cache::requeue_running_tasks(&transaction, run_id)?;
        if recovered > 0 {
            repo::person_cache::requeue_analysis_run(&transaction, run_id)?;
        }
        transaction.commit().map_err(StoreError::from)?;
        Ok(recovered)
    }

    /// Reobserves the source after inference, then commits derived features
    /// and progress with generation and claim fencing in one transaction.
    pub fn complete_person_analysis_task(
        &self,
        task: &PersonAnalysisTask,
        features: &[PersonFeature],
    ) -> Result<PersonAnalysisRun, PeopleError> {
        self.complete_person_analysis_task_with_detections(task, features, None)
    }

    /// A detection-producing stage supplies `Some`, including an empty slice
    /// when it found no instances. This atomically replaces that stage's old
    /// rebuildable detections for the asset.
    pub fn complete_person_analysis_task_with_detections(
        &self,
        task: &PersonAnalysisTask,
        features: &[PersonFeature],
        detections: Option<&[DetectedPersonInstance]>,
    ) -> Result<PersonAnalysisRun, PeopleError> {
        let observation = oxy_fs::observe_file(&task.asset_path)?;
        if observation.canonical_path != task.asset_path {
            return Err(PeopleError::PersonAnalysisConflict);
        }
        self.complete_person_analysis_task_observed(
            task,
            &observation.revision_id(),
            features,
            detections,
        )
    }

    fn complete_person_analysis_task_observed(
        &self,
        task: &PersonAnalysisTask,
        observed_source_revision: &str,
        features: &[PersonFeature],
        detections: Option<&[DetectedPersonInstance]>,
    ) -> Result<PersonAnalysisRun, PeopleError> {
        if task.run_id.is_empty()
            || task.stage_id.is_empty()
            || task.source_revision != observed_source_revision
            || !valid_fingerprint(&task.stage_fingerprint)
            || task.claim_token.is_empty()
        {
            return Err(PeopleError::PersonAnalysisConflict);
        }
        if !features.is_empty() {
            self.vector_status
                .as_ref()
                .map_err(|error| PeopleError::PersonVectorUnavailable(error.clone()))?;
        }
        for feature in features {
            validate_feature(feature)?;
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        let run = current_run(&transaction, &task.run_id)?;
        if run.folder_path != task.folder_path {
            return Err(PeopleError::PersonAnalysisConflict);
        }
        let asset_path = task.asset_path.to_string_lossy();
        let recorded = repo::person_cache::analysis_task_record(
            &transaction,
            &task.run_id,
            &asset_path,
            &task.stage_id,
        )?;
        if recorded.as_ref()
            != Some(&(
                task.source_revision.clone(),
                task.stage_fingerprint.clone(),
                i64::from(task.stage_order),
                "running".to_owned(),
                Some(task.claim_token.clone()),
            ))
        {
            return Err(PeopleError::PersonAnalysisConflict);
        }
        if let Some(detections) = detections {
            replace_stage_detections(
                &transaction,
                &DetectionStageContext {
                    folder: &run.folder_path,
                    asset: &task.asset_path,
                    source_revision: &task.source_revision,
                    producer_fingerprint: &task.stage_fingerprint,
                    pipeline_fingerprint: &run.pipeline_fingerprint,
                    run_id: &task.run_id,
                },
                detections,
            )?;
        }
        for feature in features {
            if feature.folder_path != run.folder_path
                || feature.asset_path != task.asset_path
                || feature.source_revision != task.source_revision
                || feature.pipeline_fingerprint != run.pipeline_fingerprint
                || feature.producer_fingerprint != task.stage_fingerprint
            {
                return Err(PeopleError::PersonAnalysisConflict);
            }
            write_feature(&transaction, feature)?;
        }
        repo::person_cache::complete_analysis_task(
            &transaction,
            &task.run_id,
            &asset_path,
            &task.stage_id,
        )?;
        repo::person_cache::bump_completed_tasks(&transaction, &task.run_id)?;
        let updated = repo::person_cache::analysis_run(&transaction, &task.run_id)?
            .ok_or(PeopleError::MissingPersonAnalysis)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(updated)
    }

    /// Records a terminal stage failure and skips dependent stages for the
    /// same asset. Other assets may continue; the run finishes as failed.
    pub fn fail_person_analysis_task(
        &self,
        task: &PersonAnalysisTask,
        error: &str,
    ) -> Result<PersonAnalysisRun, PeopleError> {
        if error.trim().is_empty() || error.len() > 2048 {
            return Err(PeopleError::InvalidPersonAnalysis);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        let run = current_run(&transaction, &task.run_id)?;
        if run.folder_path != task.folder_path {
            return Err(PeopleError::PersonAnalysisConflict);
        }
        let asset_path = task.asset_path.to_string_lossy();
        let changed =
            repo::person_cache::fail_analysis_task(&transaction, task, &asset_path, error)?;
        if changed != 1 {
            return Err(PeopleError::PersonAnalysisConflict);
        }
        let skipped = repo::person_cache::fail_dependent_tasks(
            &transaction,
            &task.run_id,
            &asset_path,
            i64::from(task.stage_order),
        )?;
        repo::person_cache::bump_failed_tasks(&transaction, &task.run_id, 1 + skipped as i64)?;
        let updated = repo::person_cache::analysis_run(&transaction, &task.run_id)?
            .ok_or(PeopleError::MissingPersonAnalysis)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(updated)
    }

    /// A worker must check this inside the same write transaction that commits
    /// its result. This read helper is only for scheduling and diagnostics.
    pub fn person_analysis_is_current(&self, run_id: &str) -> Result<bool, PeopleError> {
        Ok(repo::person_cache::analysis_is_current(
            &self.store.read(),
            run_id,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use oxy_domain::FeatureModality;
    use std::path::{Path, PathBuf};

    fn input(request_id: &str) -> BeginPersonAnalysis {
        BeginPersonAnalysis {
            folder_path: PathBuf::from("/photos"),
            pipeline_id: "person-v1".into(),
            pipeline_fingerprint: "a".repeat(64),
            request_id: request_id.into(),
        }
    }

    #[test]
    fn starts_idempotently_and_fences_superseded_runs() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let people = testing::open(&database);
        let first = people.begin_person_analysis(&input("start-1")).unwrap();
        assert_eq!(first.state, PersonAnalysisState::Queued);
        assert_eq!(first.generation, 1);
        assert_eq!(
            people
                .begin_person_analysis(&input("start-1"))
                .unwrap()
                .run_id,
            first.run_id
        );
        let second = people.begin_person_analysis(&input("start-2")).unwrap();
        assert_eq!(second.generation, 2);
        assert!(!people.person_analysis_is_current(&first.run_id).unwrap());
        assert!(people.person_analysis_is_current(&second.run_id).unwrap());
        assert_eq!(
            people.get_person_analysis(&first.run_id).unwrap().state,
            PersonAnalysisState::Cancelled
        );
        drop(people);
        let reopened = testing::open(&database);
        assert_eq!(
            reopened.get_person_analysis(&second.run_id).unwrap(),
            second
        );
        assert_eq!(
            reopened
                .cancel_person_analysis(&second.run_id, "cancel-2")
                .unwrap()
                .state,
            PersonAnalysisState::Cancelled
        );
        assert_eq!(
            reopened
                .cancel_person_analysis(&second.run_id, "cancel-2")
                .unwrap()
                .state,
            PersonAnalysisState::Cancelled
        );
        assert!(!reopened.person_analysis_is_current(&second.run_id).unwrap());
    }

    #[test]
    fn request_reuse_with_different_intent_conflicts() {
        let people = testing::in_memory();
        let first = people.begin_person_analysis(&input("same")).unwrap();
        let mut changed = input("same");
        changed.pipeline_id = "other".into();
        assert!(matches!(
            people.begin_person_analysis(&changed),
            Err(PeopleError::PersonAnalysisConflict)
        ));
        assert!(matches!(
            people.cancel_person_analysis(&first.run_id, "same"),
            Err(PeopleError::PersonAnalysisConflict)
        ));
        assert!(matches!(
            people.cancel_person_analysis("missing", "cancel"),
            Err(PeopleError::MissingPersonAnalysis)
        ));
    }

    #[test]
    fn task_batches_are_idempotent_and_claims_respect_stage_order() {
        let people = testing::in_memory();
        let run = people.begin_person_analysis(&input("start")).unwrap();
        let tasks = [
            PersonAnalysisTaskInput {
                asset_path: PathBuf::from("/photos/a.jpg"),
                source_revision: "10:20".into(),
                stage_id: "detect".into(),
                stage_fingerprint: "b".repeat(64),
                stage_order: 0,
            },
            PersonAnalysisTaskInput {
                asset_path: PathBuf::from("/photos/a.jpg"),
                source_revision: "10:20".into(),
                stage_id: "encode".into(),
                stage_fingerprint: "c".repeat(64),
                stage_order: 1,
            },
        ];
        assert_eq!(
            people
                .enqueue_person_analysis_tasks(&run.run_id, &tasks)
                .unwrap(),
            2
        );
        assert_eq!(
            people
                .enqueue_person_analysis_tasks(&run.run_id, &tasks)
                .unwrap(),
            0
        );
        assert_eq!(
            people
                .seal_person_analysis_tasks(&run.run_id)
                .unwrap()
                .total_tasks,
            2
        );
        assert_eq!(
            people
                .claim_person_analysis_task(&run.run_id)
                .unwrap()
                .unwrap()
                .stage_id,
            "detect"
        );
        assert!(
            people
                .claim_person_analysis_task(&run.run_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            people.get_person_analysis(&run.run_id).unwrap().state,
            PersonAnalysisState::Running
        );
        people
            .cancel_person_analysis(&run.run_id, "cancel")
            .unwrap();
        assert!(matches!(
            people.claim_person_analysis_task(&run.run_id),
            Err(PeopleError::PersonAnalysisConflict)
        ));
    }

    #[test]
    fn sealing_an_empty_run_completes_without_marking_any_failure() {
        let people = testing::in_memory();
        let run = people.begin_person_analysis(&input("start-empty")).unwrap();
        let sealed = people.seal_person_analysis_tasks(&run.run_id).unwrap();
        assert_eq!(sealed.state, PersonAnalysisState::Completed);
        assert!(sealed.enumeration_complete);
        assert_eq!(sealed.total_tasks, 0);
    }

    #[test]
    fn conflicting_task_batch_rolls_back_before_count_changes() {
        let people = testing::in_memory();
        let run = people
            .begin_person_analysis(&input("start-conflict"))
            .unwrap();
        let first = PersonAnalysisTaskInput {
            asset_path: PathBuf::from("/photos/a.jpg"),
            source_revision: "10:20".into(),
            stage_id: "detect".into(),
            stage_fingerprint: "b".repeat(64),
            stage_order: 0,
        };
        let mut duplicate_order = first.clone();
        duplicate_order.stage_id = "encode".into();
        assert!(matches!(
            people.enqueue_person_analysis_tasks(&run.run_id, &[first.clone(), duplicate_order]),
            Err(PeopleError::PersonAnalysisConflict)
        ));
        assert_eq!(
            people.get_person_analysis(&run.run_id).unwrap().total_tasks,
            0
        );
        assert_eq!(
            people
                .enqueue_person_analysis_tasks(&run.run_id, &[first])
                .unwrap(),
            1
        );
    }

    #[test]
    fn feature_commit_is_atomic_and_fenced_by_source_task_and_generation() {
        let people = testing::in_memory();
        let run = people
            .begin_person_analysis(&input("commit-start"))
            .unwrap();
        let task_input = PersonAnalysisTaskInput {
            asset_path: PathBuf::from("/photos/a.jpg"),
            source_revision: "10:20".into(),
            stage_id: "encode".into(),
            stage_fingerprint: "b".repeat(64),
            stage_order: 0,
        };
        people
            .enqueue_person_analysis_tasks(&run.run_id, &[task_input])
            .unwrap();
        people.seal_person_analysis_tasks(&run.run_id).unwrap();
        let task = people
            .claim_person_analysis_task(&run.run_id)
            .unwrap()
            .unwrap();
        let feature = PersonFeature {
            folder_path: PathBuf::from("/photos"),
            asset_path: task.asset_path.clone(),
            instance_id: "detected-face-1".into(),
            source_revision: task.source_revision.clone(),
            feature_space_id: "face-v1".into(),
            modality: FeatureModality::Face,
            producer_fingerprint: task.stage_fingerprint.clone(),
            pipeline_fingerprint: run.pipeline_fingerprint.clone(),
            values: vec![1.0, 0.0, 0.0],
        };
        assert!(matches!(
            people.complete_person_analysis_task_observed(
                &task,
                "11:21",
                std::slice::from_ref(&feature),
                None,
            ),
            Err(PeopleError::PersonAnalysisConflict)
        ));
        let mut wrong_feature = feature.clone();
        wrong_feature.asset_path = PathBuf::from("/photos/b.jpg");
        assert!(matches!(
            people.complete_person_analysis_task_observed(&task, "10:20", &[wrong_feature], None),
            Err(PeopleError::PersonAnalysisConflict)
        ));
        assert!(
            people
                .search_person_features(
                    Path::new("/photos"),
                    "face-v1",
                    &[1.0, 0.0, 0.0],
                    0.0,
                    (10, 0),
                    &[(task.asset_path.clone(), task.source_revision.clone())],
                )
                .unwrap()
                .is_empty()
        );
        let completed = people
            .complete_person_analysis_task_observed(&task, "10:20", &[feature], None)
            .unwrap();
        assert_eq!(completed.state, PersonAnalysisState::Completed);
        assert_eq!(completed.completed_tasks, 1);
        assert!(matches!(
            people.complete_person_analysis_task_observed(&task, "10:20", &[], None),
            Err(PeopleError::PersonAnalysisConflict)
        ));
        let newer = people
            .begin_person_analysis(&input("commit-newer"))
            .unwrap();
        assert_eq!(newer.generation, run.generation + 1);
        people
            .enqueue_person_analysis_tasks(
                &newer.run_id,
                &[PersonAnalysisTaskInput {
                    asset_path: PathBuf::from("/photos/b.jpg"),
                    source_revision: "10:20".into(),
                    stage_id: "encode".into(),
                    stage_fingerprint: "b".repeat(64),
                    stage_order: 0,
                }],
            )
            .unwrap();
        let late_task = people
            .claim_person_analysis_task(&newer.run_id)
            .unwrap()
            .unwrap();
        people
            .begin_person_analysis(&input("commit-newest"))
            .unwrap();
        assert!(matches!(
            people.complete_person_analysis_task_observed(&late_task, "10:20", &[], None),
            Err(PeopleError::PersonAnalysisConflict)
        ));
    }

    #[test]
    fn failed_stage_skips_dependents_and_finishes_run() {
        let people = testing::in_memory();
        let run = people.begin_person_analysis(&input("fail-start")).unwrap();
        let tasks = [
            PersonAnalysisTaskInput {
                asset_path: PathBuf::from("/photos/a.jpg"),
                source_revision: "10:20".into(),
                stage_id: "detect".into(),
                stage_fingerprint: "b".repeat(64),
                stage_order: 0,
            },
            PersonAnalysisTaskInput {
                asset_path: PathBuf::from("/photos/a.jpg"),
                source_revision: "10:20".into(),
                stage_id: "encode".into(),
                stage_fingerprint: "c".repeat(64),
                stage_order: 1,
            },
        ];
        people
            .enqueue_person_analysis_tasks(&run.run_id, &tasks)
            .unwrap();
        people.seal_person_analysis_tasks(&run.run_id).unwrap();
        let claimed = people
            .claim_person_analysis_task(&run.run_id)
            .unwrap()
            .unwrap();
        let failed = people
            .fail_person_analysis_task(&claimed, "decode failed")
            .unwrap();
        assert_eq!(failed.state, PersonAnalysisState::Failed);
        assert_eq!(failed.failed_tasks, 2);
        assert!(matches!(
            people.fail_person_analysis_task(&claimed, "decode failed"),
            Err(PeopleError::PersonAnalysisConflict)
        ));
    }

    #[test]
    fn sealing_after_all_tasks_failed_finishes_the_run() {
        let people = testing::in_memory();
        let run = people.begin_person_analysis(&input("late-seal")).unwrap();
        people
            .enqueue_person_analysis_tasks(
                &run.run_id,
                &[PersonAnalysisTaskInput {
                    asset_path: PathBuf::from("/photos/a.jpg"),
                    source_revision: "10:20".into(),
                    stage_id: "detect".into(),
                    stage_fingerprint: "b".repeat(64),
                    stage_order: 0,
                }],
            )
            .unwrap();
        let claimed = people
            .claim_person_analysis_task(&run.run_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            people
                .fail_person_analysis_task(&claimed, "decode failed")
                .unwrap()
                .state,
            PersonAnalysisState::Running
        );
        assert_eq!(
            people
                .seal_person_analysis_tasks(&run.run_id)
                .unwrap()
                .state,
            PersonAnalysisState::Failed
        );
    }

    #[test]
    fn recovered_claim_rejects_late_worker_and_survives_reopen() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let people = testing::open(&database);
        let run = people
            .begin_person_analysis(&input("recover-start"))
            .unwrap();
        people
            .enqueue_person_analysis_tasks(
                &run.run_id,
                &[PersonAnalysisTaskInput {
                    asset_path: PathBuf::from("/photos/a.jpg"),
                    source_revision: "10:20".into(),
                    stage_id: "detect".into(),
                    stage_fingerprint: "b".repeat(64),
                    stage_order: 0,
                }],
            )
            .unwrap();
        people.seal_person_analysis_tasks(&run.run_id).unwrap();
        let old_claim = people
            .claim_person_analysis_task(&run.run_id)
            .unwrap()
            .unwrap();
        assert!(!old_claim.claim_token.is_empty());
        drop(people);

        let reopened = testing::open(&database);
        assert_eq!(
            reopened.recover_person_analysis_tasks(&run.run_id).unwrap(),
            1
        );
        assert_eq!(
            reopened.recover_person_analysis_tasks(&run.run_id).unwrap(),
            0
        );
        let new_claim = reopened
            .claim_person_analysis_task(&run.run_id)
            .unwrap()
            .unwrap();
        assert_ne!(old_claim.claim_token, new_claim.claim_token);
        assert!(matches!(
            reopened.complete_person_analysis_task_observed(&old_claim, "10:20", &[], None),
            Err(PeopleError::PersonAnalysisConflict)
        ));
        assert!(matches!(
            reopened.fail_person_analysis_task(&old_claim, "late error"),
            Err(PeopleError::PersonAnalysisConflict)
        ));
        let completed = reopened
            .complete_person_analysis_task_observed(&new_claim, "10:20", &[], None)
            .unwrap();
        assert_eq!(completed.state, PersonAnalysisState::Completed);
        assert_eq!(completed.completed_tasks, 1);
    }

    #[test]
    fn opening_previous_task_schema_adds_claim_token_without_losing_rows() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        // A stale schema can only be staged outside the store's own step, so
        // this test writes the old table shape directly.
        let connection = oxy_store::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE person_analysis_tasks (
                   run_id TEXT NOT NULL, asset_path TEXT NOT NULL,
                   source_revision TEXT NOT NULL, stage_id TEXT NOT NULL,
                   stage_fingerprint TEXT NOT NULL, stage_order INTEGER NOT NULL,
                   state TEXT NOT NULL, error TEXT,
                   PRIMARY KEY(run_id,asset_path,stage_id),
                   UNIQUE(run_id,asset_path,stage_order));
                 INSERT INTO person_analysis_tasks VALUES
                   ('old-run','/photos/a.jpg','10:20','detect','old',0,'completed',NULL);",
            )
            .unwrap();
        drop(connection);
        let people = testing::open(&database);
        let connection = people.store.write();
        let found: (String, Option<String>) = connection
            .query_row(
                "SELECT state,claim_token FROM person_analysis_tasks WHERE run_id='old-run'",
                [],
                |row| Ok((row.get("state")?, row.get("claim_token")?)),
            )
            .unwrap();
        assert_eq!(found, ("completed".into(), None));
    }

    #[test]
    fn public_commit_reobserves_real_source_and_rejects_changed_file() {
        let temporary = tempfile::tempdir().unwrap();
        let folder = temporary.path().canonicalize().unwrap();
        let asset = folder.join("a.jpg");
        std::fs::write(&asset, b"first version").unwrap();
        let first_revision = oxy_fs::observe_file(&asset).unwrap().revision_id();
        let people = testing::in_memory();
        let mut begin = input("source-first");
        begin.folder_path = folder;
        let first = people.begin_person_analysis(&begin).unwrap();
        people
            .enqueue_person_analysis_tasks(
                &first.run_id,
                &[PersonAnalysisTaskInput {
                    asset_path: asset.clone(),
                    source_revision: first_revision,
                    stage_id: "detect".into(),
                    stage_fingerprint: "b".repeat(64),
                    stage_order: 0,
                }],
            )
            .unwrap();
        let stale_task = people
            .claim_person_analysis_task(&first.run_id)
            .unwrap()
            .unwrap();
        std::fs::write(&asset, b"second, longer version").unwrap();
        assert!(matches!(
            people.complete_person_analysis_task(&stale_task, &[]),
            Err(PeopleError::PersonAnalysisConflict)
        ));

        begin.request_id = "source-second".into();
        let second = people.begin_person_analysis(&begin).unwrap();
        people
            .enqueue_person_analysis_tasks(
                &second.run_id,
                &[PersonAnalysisTaskInput {
                    asset_path: asset.clone(),
                    source_revision: oxy_fs::observe_file(&asset).unwrap().revision_id(),
                    stage_id: "detect".into(),
                    stage_fingerprint: "b".repeat(64),
                    stage_order: 0,
                }],
            )
            .unwrap();
        people.seal_person_analysis_tasks(&second.run_id).unwrap();
        let current_task = people
            .claim_person_analysis_task(&second.run_id)
            .unwrap()
            .unwrap();
        let completed = people
            .complete_person_analysis_task(&current_task, &[])
            .unwrap();
        assert_eq!(completed.state, PersonAnalysisState::Completed);
    }
}
