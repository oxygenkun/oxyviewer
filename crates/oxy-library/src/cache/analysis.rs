//! Persistent generation fence for explicit person analysis runs. Worker tasks
//! will use the head row again inside their result-commit transaction.

use crate::cache::detections::{DetectionStageContext, replace_stage_detections};
use crate::cache::features::{PersonFeature, validate_feature, write_feature};
use crate::{Library, LibraryError};
use oxy_domain::{
    BeginPersonAnalysis, DetectedPersonInstance, PersonAnalysisRun, PersonAnalysisState,
    PersonAnalysisTask, PersonAnalysisTaskInput,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use std::path::PathBuf;

oxy_store::table::tables! {
    clear person_analysis_heads = "folder_path TEXT PRIMARY KEY, generation INTEGER NOT NULL, run_id TEXT NOT NULL";
    clear person_analysis_runs =
        "run_id TEXT PRIMARY KEY,
        folder_path TEXT NOT NULL,
        pipeline_id TEXT NOT NULL,
        pipeline_fingerprint TEXT NOT NULL,
        generation INTEGER NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('queued','running','completed','failed','cancelled')),
        enumeration_complete INTEGER NOT NULL DEFAULT 0,
        total_tasks INTEGER NOT NULL DEFAULT 0,
        completed_tasks INTEGER NOT NULL DEFAULT 0,
        failed_tasks INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch())";
    clear person_analysis_requests =
        "request_id TEXT PRIMARY KEY,
        operation TEXT NOT NULL,
        run_id TEXT NOT NULL REFERENCES person_analysis_runs(run_id)";
    clear person_analysis_tasks =
        "run_id TEXT NOT NULL REFERENCES person_analysis_runs(run_id),
        asset_path TEXT NOT NULL,
        source_revision TEXT NOT NULL,
        stage_id TEXT NOT NULL,
        stage_fingerprint TEXT NOT NULL,
        stage_order INTEGER NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('queued','running','completed','failed')),
        claim_token TEXT,
        error TEXT,
        PRIMARY KEY(run_id,asset_path,stage_id),
        UNIQUE(run_id,asset_path,stage_order)";
}

pub(crate) fn ensure_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    oxy_store::table::create_all(connection, DEFS)?;
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS person_analysis_runs_folder
           ON person_analysis_runs(folder_path,generation DESC);
         CREATE INDEX IF NOT EXISTS person_analysis_tasks_next
           ON person_analysis_tasks(run_id,state,asset_path,stage_order);",
    )?;
    let has_claim_token: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('person_analysis_tasks')
           WHERE name='claim_token') AS present",
        [],
        |row| row.get("present"),
    )?;
    if !has_claim_token {
        connection.execute(
            "ALTER TABLE person_analysis_tasks ADD COLUMN claim_token TEXT",
            [],
        )?;
    }
    Ok(())
}

/// Empties every table declared above. [`oxy_store::table::clear_all`] walks the
/// declaration backwards, so tasks and requests go before the runs they
/// reference.
pub(crate) fn clear(connection: &Connection) -> Result<(), rusqlite::Error> {
    oxy_store::table::clear_all(connection, DEFS)
}

fn row_run(row: &Row<'_>) -> rusqlite::Result<PersonAnalysisRun> {
    let state: String = row.get("state")?;
    let state = match state.as_str() {
        "queued" => PersonAnalysisState::Queued,
        "running" => PersonAnalysisState::Running,
        "completed" => PersonAnalysisState::Completed,
        "failed" => PersonAnalysisState::Failed,
        "cancelled" => PersonAnalysisState::Cancelled,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(PersonAnalysisRun {
        run_id: row.get("run_id")?,
        folder_path: PathBuf::from(row.get::<_, String>("folder_path")?),
        pipeline_id: row.get("pipeline_id")?,
        pipeline_fingerprint: row.get("pipeline_fingerprint")?,
        generation: row.get("generation")?,
        state,
        enumeration_complete: row.get::<_, i64>("enumeration_complete")? != 0,
        total_tasks: row.get::<_, i64>("total_tasks")? as u64,
        completed_tasks: row.get::<_, i64>("completed_tasks")? as u64,
        failed_tasks: row.get::<_, i64>("failed_tasks")? as u64,
    })
}

const RUN_COLUMNS: &str = "run_id,folder_path,pipeline_id,pipeline_fingerprint,generation,state,
                           enumeration_complete,total_tasks,completed_tasks,failed_tasks";

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn current_run(
    transaction: &Transaction<'_>,
    run_id: &str,
) -> Result<PersonAnalysisRun, LibraryError> {
    let run: PersonAnalysisRun = transaction
        .query_row(
            &format!("SELECT {RUN_COLUMNS} FROM person_analysis_runs WHERE run_id=?1"),
            [run_id],
            row_run,
        )
        .optional()?
        .ok_or(LibraryError::MissingPersonAnalysis)?;
    let is_head: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM person_analysis_heads
           WHERE folder_path=?1 AND run_id=?2 AND generation=?3) AS present",
        params![
            run.folder_path.to_string_lossy(),
            run.run_id,
            run.generation
        ],
        |row| row.get("present"),
    )?;
    if !is_head
        || !matches!(
            run.state,
            PersonAnalysisState::Queued | PersonAnalysisState::Running
        )
    {
        return Err(LibraryError::PersonAnalysisConflict);
    }
    Ok(run)
}

fn row_task(row: &Row<'_>) -> rusqlite::Result<PersonAnalysisTask> {
    Ok(PersonAnalysisTask {
        run_id: row.get("run_id")?,
        folder_path: PathBuf::from(row.get::<_, String>("folder_path")?),
        asset_path: PathBuf::from(row.get::<_, String>("asset_path")?),
        source_revision: row.get("source_revision")?,
        stage_id: row.get("stage_id")?,
        stage_fingerprint: row.get("stage_fingerprint")?,
        stage_order: row.get::<_, i64>("stage_order")? as u32,
        claim_token: String::new(),
    })
}

impl Library {
    /// Starts a new folder generation. A replay of the same request returns its
    /// original run; a new request supersedes the previous active run.
    pub fn begin_person_analysis(
        &self,
        input: &BeginPersonAnalysis,
    ) -> Result<PersonAnalysisRun, LibraryError> {
        if input.folder_path.as_os_str().is_empty()
            || input.pipeline_id.trim().is_empty()
            || !valid_fingerprint(&input.pipeline_fingerprint)
            || input.request_id.trim().is_empty()
        {
            return Err(LibraryError::InvalidPersonAnalysis);
        }
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let replay: Option<(String, String)> = transaction
            .query_row(
                "SELECT operation,run_id FROM person_analysis_requests WHERE request_id=?1",
                [&input.request_id],
                |row| Ok((row.get("operation")?, row.get("run_id")?)),
            )
            .optional()?;
        if let Some((operation, run_id)) = replay {
            if operation != "begin" {
                return Err(LibraryError::PersonAnalysisConflict);
            }
            let run = transaction.query_row(
                &format!("SELECT {RUN_COLUMNS} FROM person_analysis_runs WHERE run_id=?1"),
                [&run_id],
                row_run,
            )?;
            if run.folder_path != input.folder_path
                || run.pipeline_id != input.pipeline_id
                || run.pipeline_fingerprint != input.pipeline_fingerprint
            {
                return Err(LibraryError::PersonAnalysisConflict);
            }
            return Ok(run);
        }
        let folder = input.folder_path.to_string_lossy();
        let previous: Option<(i64, String)> = transaction
            .query_row(
                "SELECT generation,run_id FROM person_analysis_heads WHERE folder_path=?1",
                [folder.as_ref()],
                |row| Ok((row.get("generation")?, row.get("run_id")?)),
            )
            .optional()?;
        let generation = previous
            .as_ref()
            .map_or(Some(1), |(value, _)| value.checked_add(1))
            .ok_or(LibraryError::PersonAnalysisConflict)?;
        if let Some((_, previous_id)) = previous {
            transaction.execute(
                "UPDATE person_analysis_runs SET state='cancelled',updated_at=unixepoch()
                 WHERE run_id=?1 AND state IN ('queued','running')",
                [&previous_id],
            )?;
        }
        let run_id: String =
            transaction.query_row("SELECT lower(hex(randomblob(16))) AS id", [], |row| {
                row.get("id")
            })?;
        transaction.execute(
            "INSERT INTO person_analysis_runs(run_id,folder_path,pipeline_id,pipeline_fingerprint,generation,state)
             VALUES (?1,?2,?3,?4,?5,'queued')",
            params![run_id,folder,input.pipeline_id,input.pipeline_fingerprint,generation],
        )?;
        transaction.execute(
            "INSERT INTO person_analysis_heads(folder_path,generation,run_id) VALUES (?1,?2,?3)
             ON CONFLICT(folder_path) DO UPDATE SET generation=excluded.generation,run_id=excluded.run_id",
            params![folder,generation,run_id],
        )?;
        transaction.execute(
            "INSERT INTO person_analysis_requests(request_id,operation,run_id) VALUES (?1,'begin',?2)",
            params![input.request_id,run_id],
        )?;
        let run = transaction.query_row(
            &format!("SELECT {RUN_COLUMNS} FROM person_analysis_runs WHERE run_id=?1"),
            [&run_id],
            row_run,
        )?;
        transaction.commit()?;
        Ok(run)
    }

    pub fn get_person_analysis(&self, run_id: &str) -> Result<PersonAnalysisRun, LibraryError> {
        let connection = self.read_connection();
        connection
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM person_analysis_runs WHERE run_id=?1"),
                [run_id],
                row_run,
            )
            .optional()?
            .ok_or(LibraryError::MissingPersonAnalysis)
    }

    pub fn cancel_person_analysis(
        &self,
        run_id: &str,
        request_id: &str,
    ) -> Result<PersonAnalysisRun, LibraryError> {
        if run_id.is_empty() || request_id.trim().is_empty() {
            return Err(LibraryError::InvalidPersonAnalysis);
        }
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let replay: Option<(String, String)> = transaction
            .query_row(
                "SELECT operation,run_id FROM person_analysis_requests WHERE request_id=?1",
                [request_id],
                |row| Ok((row.get("operation")?, row.get("run_id")?)),
            )
            .optional()?;
        if let Some((operation, prior_id)) = replay {
            if operation != "cancel" || prior_id != run_id {
                return Err(LibraryError::PersonAnalysisConflict);
            }
        } else {
            let changed = transaction.execute(
                "UPDATE person_analysis_runs SET state='cancelled',updated_at=unixepoch()
                 WHERE run_id=?1 AND state IN ('queued','running')",
                [run_id],
            )?;
            if changed == 0 {
                let exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM person_analysis_runs WHERE run_id=?1) AS present",
                    [run_id],
                    |row| row.get("present"),
                )?;
                if !exists {
                    return Err(LibraryError::MissingPersonAnalysis);
                }
            }
            transaction.execute(
                "INSERT INTO person_analysis_requests(request_id,operation,run_id) VALUES (?1,'cancel',?2)",
                params![request_id,run_id],
            )?;
        }
        let run = transaction.query_row(
            &format!("SELECT {RUN_COLUMNS} FROM person_analysis_runs WHERE run_id=?1"),
            [run_id],
            row_run,
        )?;
        transaction.commit()?;
        Ok(run)
    }

    /// Registers at most one small batch from the current `oxy-fs` snapshot.
    /// Replaying identical tasks is safe; changed inputs require a new run.
    pub fn enqueue_person_analysis_tasks(
        &self,
        run_id: &str,
        tasks: &[PersonAnalysisTaskInput],
    ) -> Result<usize, LibraryError> {
        if tasks.is_empty() || tasks.len() > 256 {
            return Err(LibraryError::InvalidPersonAnalysis);
        }
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let run = current_run(&transaction, run_id)?;
        if run.enumeration_complete {
            return Err(LibraryError::PersonAnalysisConflict);
        }
        let mut added = 0usize;
        for task in tasks {
            if task.asset_path.parent() != Some(run.folder_path.as_path())
                || task.source_revision.is_empty()
                || task.stage_id.trim().is_empty()
                || !valid_fingerprint(&task.stage_fingerprint)
            {
                return Err(LibraryError::InvalidPersonAnalysis);
            }
            let asset_path = task.asset_path.to_string_lossy();
            let existing: Option<(String, String, i64)> = transaction
                .query_row(
                    "SELECT source_revision,stage_fingerprint,stage_order
                     FROM person_analysis_tasks WHERE run_id=?1 AND asset_path=?2 AND stage_id=?3",
                    params![run_id, asset_path, task.stage_id],
                    |row| {
                        Ok((
                            row.get("source_revision")?,
                            row.get("stage_fingerprint")?,
                            row.get("stage_order")?,
                        ))
                    },
                )
                .optional()?;
            if let Some((source, fingerprint, order)) = existing {
                if source != task.source_revision
                    || fingerprint != task.stage_fingerprint
                    || order != i64::from(task.stage_order)
                {
                    return Err(LibraryError::PersonAnalysisConflict);
                }
                continue;
            }
            let order_taken: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM person_analysis_tasks
                   WHERE run_id=?1 AND asset_path=?2 AND stage_order=?3) AS present",
                params![run_id, asset_path, task.stage_order],
                |row| row.get("present"),
            )?;
            if order_taken {
                return Err(LibraryError::PersonAnalysisConflict);
            }
            transaction.execute(
                "INSERT INTO person_analysis_tasks(run_id,asset_path,source_revision,
                    stage_id,stage_fingerprint,stage_order,state)
                 VALUES (?1,?2,?3,?4,?5,?6,'queued')",
                params![
                    run_id,
                    asset_path,
                    task.source_revision,
                    task.stage_id,
                    task.stage_fingerprint,
                    task.stage_order
                ],
            )?;
            added += 1;
        }
        transaction.execute(
            "UPDATE person_analysis_runs SET total_tasks=total_tasks+?2,updated_at=unixepoch()
             WHERE run_id=?1",
            params![run_id, added as i64],
        )?;
        transaction.commit()?;
        Ok(added)
    }

    pub fn seal_person_analysis_tasks(
        &self,
        run_id: &str,
    ) -> Result<PersonAnalysisRun, LibraryError> {
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        current_run(&transaction, run_id)?;
        transaction.execute(
            "UPDATE person_analysis_runs SET enumeration_complete=1,
               state=CASE WHEN completed_tasks+failed_tasks=total_tasks
                 THEN CASE WHEN failed_tasks=0 THEN 'completed' ELSE 'failed' END
                 ELSE state END,
               updated_at=unixepoch() WHERE run_id=?1",
            [run_id],
        )?;
        let run = transaction.query_row(
            &format!("SELECT {RUN_COLUMNS} FROM person_analysis_runs WHERE run_id=?1"),
            [run_id],
            row_run,
        )?;
        transaction.commit()?;
        Ok(run)
    }

    /// Claims a task only after earlier stages for that asset have completed.
    pub fn claim_person_analysis_task(
        &self,
        run_id: &str,
    ) -> Result<Option<PersonAnalysisTask>, LibraryError> {
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        current_run(&transaction, run_id)?;
        let mut task = transaction
            .query_row(
                "SELECT t.run_id,r.folder_path,t.asset_path,t.source_revision,t.stage_id,
                        t.stage_fingerprint,t.stage_order
                 FROM person_analysis_tasks t JOIN person_analysis_runs r ON r.run_id=t.run_id
                 WHERE t.run_id=?1 AND t.state='queued'
                   AND NOT EXISTS (SELECT 1 FROM person_analysis_tasks earlier
                     WHERE earlier.run_id=t.run_id AND earlier.asset_path=t.asset_path
                       AND earlier.stage_order<t.stage_order AND earlier.state!='completed')
                 ORDER BY t.asset_path,t.stage_order LIMIT 1",
                [run_id],
                row_task,
            )
            .optional()?;
        if let Some(task) = &mut task {
            let claim_token: String =
                transaction.query_row("SELECT lower(hex(randomblob(16))) AS id", [], |row| {
                    row.get("id")
                })?;
            let changed = transaction.execute(
                "UPDATE person_analysis_tasks SET state='running',claim_token=?4
                 WHERE run_id=?1 AND asset_path=?2 AND stage_id=?3 AND state='queued'",
                params![
                    run_id,
                    task.asset_path.to_string_lossy(),
                    task.stage_id,
                    claim_token
                ],
            )?;
            if changed != 1 {
                return Err(LibraryError::PersonAnalysisConflict);
            }
            task.claim_token = claim_token;
            transaction.execute(
                "UPDATE person_analysis_runs SET state='running',updated_at=unixepoch()
                 WHERE run_id=?1 AND state='queued'",
                [run_id],
            )?;
        }
        transaction.commit()?;
        Ok(task)
    }

    /// Requeues claims left by an interrupted worker. The old claim tokens are
    /// invalidated in the same transaction, so late workers cannot publish.
    /// Call this when starting or restarting the worker for a persisted run.
    pub fn recover_person_analysis_tasks(&self, run_id: &str) -> Result<usize, LibraryError> {
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        current_run(&transaction, run_id)?;
        let recovered = transaction.execute(
            "UPDATE person_analysis_tasks SET state='queued',claim_token=NULL,error=NULL
             WHERE run_id=?1 AND state='running'",
            [run_id],
        )?;
        if recovered > 0 {
            transaction.execute(
                "UPDATE person_analysis_runs SET state='queued',updated_at=unixepoch()
                 WHERE run_id=?1",
                [run_id],
            )?;
        }
        transaction.commit()?;
        Ok(recovered)
    }

    /// Reobserves the source after inference, then commits derived features
    /// and progress with generation and claim fencing in one transaction.
    pub fn complete_person_analysis_task(
        &self,
        task: &PersonAnalysisTask,
        features: &[PersonFeature],
    ) -> Result<PersonAnalysisRun, LibraryError> {
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
    ) -> Result<PersonAnalysisRun, LibraryError> {
        let observation = oxy_fs::observe_file(&task.asset_path)?;
        if observation.canonical_path != task.asset_path {
            return Err(LibraryError::PersonAnalysisConflict);
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
    ) -> Result<PersonAnalysisRun, LibraryError> {
        if task.run_id.is_empty()
            || task.stage_id.is_empty()
            || task.source_revision != observed_source_revision
            || !valid_fingerprint(&task.stage_fingerprint)
            || task.claim_token.is_empty()
        {
            return Err(LibraryError::PersonAnalysisConflict);
        }
        if !features.is_empty() {
            self.vector_status
                .as_ref()
                .map_err(|error| LibraryError::PersonVectorUnavailable(error.clone()))?;
        }
        for feature in features {
            validate_feature(feature)?;
        }
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let run = current_run(&transaction, &task.run_id)?;
        if run.folder_path != task.folder_path {
            return Err(LibraryError::PersonAnalysisConflict);
        }
        let recorded: Option<(String, String, i64, String, Option<String>)> = transaction
            .query_row(
                "SELECT source_revision,stage_fingerprint,stage_order,state,claim_token
                 FROM person_analysis_tasks WHERE run_id=?1 AND asset_path=?2 AND stage_id=?3",
                params![
                    task.run_id,
                    task.asset_path.to_string_lossy(),
                    task.stage_id
                ],
                |row| {
                    Ok((
                        row.get("source_revision")?,
                        row.get("stage_fingerprint")?,
                        row.get("stage_order")?,
                        row.get("state")?,
                        row.get("claim_token")?,
                    ))
                },
            )
            .optional()?;
        if recorded.as_ref()
            != Some(&(
                task.source_revision.clone(),
                task.stage_fingerprint.clone(),
                i64::from(task.stage_order),
                "running".to_owned(),
                Some(task.claim_token.clone()),
            ))
        {
            return Err(LibraryError::PersonAnalysisConflict);
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
                return Err(LibraryError::PersonAnalysisConflict);
            }
            write_feature(&transaction, feature)?;
        }
        transaction.execute(
            "UPDATE person_analysis_tasks SET state='completed',error=NULL
             WHERE run_id=?1 AND asset_path=?2 AND stage_id=?3",
            params![
                task.run_id,
                task.asset_path.to_string_lossy(),
                task.stage_id
            ],
        )?;
        transaction.execute(
            "UPDATE person_analysis_runs SET completed_tasks=completed_tasks+1,
               state=CASE WHEN enumeration_complete=1 AND completed_tasks+failed_tasks+1=total_tasks
                 THEN CASE WHEN failed_tasks=0 THEN 'completed' ELSE 'failed' END
                 ELSE state END, updated_at=unixepoch() WHERE run_id=?1",
            [&task.run_id],
        )?;
        let updated = transaction.query_row(
            &format!("SELECT {RUN_COLUMNS} FROM person_analysis_runs WHERE run_id=?1"),
            [&task.run_id],
            row_run,
        )?;
        transaction.commit()?;
        Ok(updated)
    }

    /// Records a terminal stage failure and skips dependent stages for the
    /// same asset. Other assets may continue; the run finishes as failed.
    pub fn fail_person_analysis_task(
        &self,
        task: &PersonAnalysisTask,
        error: &str,
    ) -> Result<PersonAnalysisRun, LibraryError> {
        if error.trim().is_empty() || error.len() > 2048 {
            return Err(LibraryError::InvalidPersonAnalysis);
        }
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let run = current_run(&transaction, &task.run_id)?;
        if run.folder_path != task.folder_path {
            return Err(LibraryError::PersonAnalysisConflict);
        }
        let changed = transaction.execute(
            "UPDATE person_analysis_tasks SET state='failed',error=?4
             WHERE run_id=?1 AND asset_path=?2 AND stage_id=?3
               AND source_revision=?5 AND stage_fingerprint=?6 AND stage_order=?7
               AND claim_token=?8 AND state='running'",
            params![
                task.run_id,
                task.asset_path.to_string_lossy(),
                task.stage_id,
                error,
                task.source_revision,
                task.stage_fingerprint,
                task.stage_order,
                task.claim_token
            ],
        )?;
        if changed != 1 {
            return Err(LibraryError::PersonAnalysisConflict);
        }
        let skipped = transaction.execute(
            "UPDATE person_analysis_tasks SET state='failed',error='earlier stage failed'
             WHERE run_id=?1 AND asset_path=?2 AND stage_order>?3 AND state='queued'",
            params![
                task.run_id,
                task.asset_path.to_string_lossy(),
                task.stage_order
            ],
        )?;
        transaction.execute(
            "UPDATE person_analysis_runs SET failed_tasks=failed_tasks+?2,
               state=CASE WHEN enumeration_complete=1
                 AND completed_tasks+failed_tasks+?2=total_tasks THEN 'failed' ELSE state END,
               updated_at=unixepoch() WHERE run_id=?1",
            params![task.run_id, 1 + skipped as i64],
        )?;
        let updated = transaction.query_row(
            &format!("SELECT {RUN_COLUMNS} FROM person_analysis_runs WHERE run_id=?1"),
            [&task.run_id],
            row_run,
        )?;
        transaction.commit()?;
        Ok(updated)
    }

    /// A worker must check this inside the same write transaction that commits
    /// its result. This read helper is only for scheduling and diagnostics.
    pub fn person_analysis_is_current(&self, run_id: &str) -> Result<bool, LibraryError> {
        let connection = self.read_connection();
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM person_analysis_runs r
               JOIN person_analysis_heads h ON h.folder_path=r.folder_path
              WHERE r.run_id=?1 AND h.run_id=r.run_id AND h.generation=r.generation
                AND r.state IN ('queued','running')) AS present",
            [run_id],
            |row| row.get("present"),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::features::FeatureModality;
    use std::path::Path;

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
        let library = Library::open(&database).unwrap();
        let first = library.begin_person_analysis(&input("start-1")).unwrap();
        assert_eq!(first.state, PersonAnalysisState::Queued);
        assert_eq!(first.generation, 1);
        assert_eq!(
            library
                .begin_person_analysis(&input("start-1"))
                .unwrap()
                .run_id,
            first.run_id
        );
        let second = library.begin_person_analysis(&input("start-2")).unwrap();
        assert_eq!(second.generation, 2);
        assert!(!library.person_analysis_is_current(&first.run_id).unwrap());
        assert!(library.person_analysis_is_current(&second.run_id).unwrap());
        assert_eq!(
            library.get_person_analysis(&first.run_id).unwrap().state,
            PersonAnalysisState::Cancelled
        );
        drop(library);
        let reopened = Library::open(&database).unwrap();
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
        let library = Library::in_memory().unwrap();
        let first = library.begin_person_analysis(&input("same")).unwrap();
        let mut changed = input("same");
        changed.pipeline_id = "other".into();
        assert!(matches!(
            library.begin_person_analysis(&changed),
            Err(LibraryError::PersonAnalysisConflict)
        ));
        assert!(matches!(
            library.cancel_person_analysis(&first.run_id, "same"),
            Err(LibraryError::PersonAnalysisConflict)
        ));
        assert!(matches!(
            library.cancel_person_analysis("missing", "cancel"),
            Err(LibraryError::MissingPersonAnalysis)
        ));
    }

    #[test]
    fn task_batches_are_idempotent_and_claims_respect_stage_order() {
        let library = Library::in_memory().unwrap();
        let run = library.begin_person_analysis(&input("start")).unwrap();
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
            library
                .enqueue_person_analysis_tasks(&run.run_id, &tasks)
                .unwrap(),
            2
        );
        assert_eq!(
            library
                .enqueue_person_analysis_tasks(&run.run_id, &tasks)
                .unwrap(),
            0
        );
        assert_eq!(
            library
                .seal_person_analysis_tasks(&run.run_id)
                .unwrap()
                .total_tasks,
            2
        );
        assert_eq!(
            library
                .claim_person_analysis_task(&run.run_id)
                .unwrap()
                .unwrap()
                .stage_id,
            "detect"
        );
        assert!(
            library
                .claim_person_analysis_task(&run.run_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            library.get_person_analysis(&run.run_id).unwrap().state,
            PersonAnalysisState::Running
        );
        library
            .cancel_person_analysis(&run.run_id, "cancel")
            .unwrap();
        assert!(matches!(
            library.claim_person_analysis_task(&run.run_id),
            Err(LibraryError::PersonAnalysisConflict)
        ));
    }

    #[test]
    fn sealing_an_empty_run_completes_without_marking_any_failure() {
        let library = Library::in_memory().unwrap();
        let run = library
            .begin_person_analysis(&input("start-empty"))
            .unwrap();
        let sealed = library.seal_person_analysis_tasks(&run.run_id).unwrap();
        assert_eq!(sealed.state, PersonAnalysisState::Completed);
        assert!(sealed.enumeration_complete);
        assert_eq!(sealed.total_tasks, 0);
    }

    #[test]
    fn conflicting_task_batch_rolls_back_before_count_changes() {
        let library = Library::in_memory().unwrap();
        let run = library
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
            library.enqueue_person_analysis_tasks(&run.run_id, &[first.clone(), duplicate_order]),
            Err(LibraryError::PersonAnalysisConflict)
        ));
        assert_eq!(
            library
                .get_person_analysis(&run.run_id)
                .unwrap()
                .total_tasks,
            0
        );
        assert_eq!(
            library
                .enqueue_person_analysis_tasks(&run.run_id, &[first])
                .unwrap(),
            1
        );
    }

    #[test]
    fn feature_commit_is_atomic_and_fenced_by_source_task_and_generation() {
        let library = Library::in_memory().unwrap();
        let run = library
            .begin_person_analysis(&input("commit-start"))
            .unwrap();
        let task_input = PersonAnalysisTaskInput {
            asset_path: PathBuf::from("/photos/a.jpg"),
            source_revision: "10:20".into(),
            stage_id: "encode".into(),
            stage_fingerprint: "b".repeat(64),
            stage_order: 0,
        };
        library
            .enqueue_person_analysis_tasks(&run.run_id, &[task_input])
            .unwrap();
        library.seal_person_analysis_tasks(&run.run_id).unwrap();
        let task = library
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
            library.complete_person_analysis_task_observed(
                &task,
                "11:21",
                std::slice::from_ref(&feature),
                None,
            ),
            Err(LibraryError::PersonAnalysisConflict)
        ));
        let mut wrong_feature = feature.clone();
        wrong_feature.asset_path = PathBuf::from("/photos/b.jpg");
        assert!(matches!(
            library.complete_person_analysis_task_observed(&task, "10:20", &[wrong_feature], None),
            Err(LibraryError::PersonAnalysisConflict)
        ));
        assert!(
            library
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
        let completed = library
            .complete_person_analysis_task_observed(&task, "10:20", &[feature], None)
            .unwrap();
        assert_eq!(completed.state, PersonAnalysisState::Completed);
        assert_eq!(completed.completed_tasks, 1);
        assert!(matches!(
            library.complete_person_analysis_task_observed(&task, "10:20", &[], None),
            Err(LibraryError::PersonAnalysisConflict)
        ));
        let newer = library
            .begin_person_analysis(&input("commit-newer"))
            .unwrap();
        assert_eq!(newer.generation, run.generation + 1);
        library
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
        let late_task = library
            .claim_person_analysis_task(&newer.run_id)
            .unwrap()
            .unwrap();
        library
            .begin_person_analysis(&input("commit-newest"))
            .unwrap();
        assert!(matches!(
            library.complete_person_analysis_task_observed(&late_task, "10:20", &[], None),
            Err(LibraryError::PersonAnalysisConflict)
        ));
    }

    #[test]
    fn failed_stage_skips_dependents_and_finishes_run() {
        let library = Library::in_memory().unwrap();
        let run = library.begin_person_analysis(&input("fail-start")).unwrap();
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
        library
            .enqueue_person_analysis_tasks(&run.run_id, &tasks)
            .unwrap();
        library.seal_person_analysis_tasks(&run.run_id).unwrap();
        let claimed = library
            .claim_person_analysis_task(&run.run_id)
            .unwrap()
            .unwrap();
        let failed = library
            .fail_person_analysis_task(&claimed, "decode failed")
            .unwrap();
        assert_eq!(failed.state, PersonAnalysisState::Failed);
        assert_eq!(failed.failed_tasks, 2);
        assert!(matches!(
            library.fail_person_analysis_task(&claimed, "decode failed"),
            Err(LibraryError::PersonAnalysisConflict)
        ));
    }

    #[test]
    fn sealing_after_all_tasks_failed_finishes_the_run() {
        let library = Library::in_memory().unwrap();
        let run = library.begin_person_analysis(&input("late-seal")).unwrap();
        library
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
        let claimed = library
            .claim_person_analysis_task(&run.run_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            library
                .fail_person_analysis_task(&claimed, "decode failed")
                .unwrap()
                .state,
            PersonAnalysisState::Running
        );
        assert_eq!(
            library
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
        let library = Library::open(&database).unwrap();
        let run = library
            .begin_person_analysis(&input("recover-start"))
            .unwrap();
        library
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
        library.seal_person_analysis_tasks(&run.run_id).unwrap();
        let old_claim = library
            .claim_person_analysis_task(&run.run_id)
            .unwrap()
            .unwrap();
        assert!(!old_claim.claim_token.is_empty());
        drop(library);

        let reopened = Library::open(&database).unwrap();
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
            Err(LibraryError::PersonAnalysisConflict)
        ));
        assert!(matches!(
            reopened.fail_person_analysis_task(&old_claim, "late error"),
            Err(LibraryError::PersonAnalysisConflict)
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
        let connection = Connection::open(&database).unwrap();
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
        let library = Library::open(&database).unwrap();
        let connection = library.write();
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
        let library = Library::in_memory().unwrap();
        let mut begin = input("source-first");
        begin.folder_path = folder;
        let first = library.begin_person_analysis(&begin).unwrap();
        library
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
        let stale_task = library
            .claim_person_analysis_task(&first.run_id)
            .unwrap()
            .unwrap();
        std::fs::write(&asset, b"second, longer version").unwrap();
        assert!(matches!(
            library.complete_person_analysis_task(&stale_task, &[]),
            Err(LibraryError::PersonAnalysisConflict)
        ));

        begin.request_id = "source-second".into();
        let second = library.begin_person_analysis(&begin).unwrap();
        library
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
        library.seal_person_analysis_tasks(&second.run_id).unwrap();
        let current_task = library
            .claim_person_analysis_task(&second.run_id)
            .unwrap()
            .unwrap();
        let completed = library
            .complete_person_analysis_task(&current_task, &[])
            .unwrap();
        assert_eq!(completed.state, PersonAnalysisState::Completed);
    }
}
