//! The rebuildable person caches: detections, feature vectors, and the analysis
//! run ledger.
//!
//! These tables are declared `cache` in `oxy_store::schema`, so emptying them
//! costs a model pass and never a name the user gave. They are still the
//! *person* domain's table, which is why the statements live beside
//! [`super::people`] rather than in [`super::library`]: the identities those
//! caches are derived for are person rows, and the row that has to be fenced
//! against a late worker is a person run.
//!
//! What is not here is the meaning of any of it. "A vector must be normalized
//! before it is stored", "a claim token from a superseded run may not publish",
//! and "a stale source revision is not a candidate" are rules, and they live in
//! `oxy-people`. What lives here is the statement each rule ends in.

use crate::StoreError;
use oxy_domain::{
    DetectedPersonInstance, FeatureModality, PersonAnalysisRun, PersonAnalysisState,
    PersonAnalysisTask, PersonFeatureMatch,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

/// The columns that make up a [`PersonAnalysisRun`].
const RUN_COLUMNS: &str = "run_id,folder_path,pipeline_id,pipeline_fingerprint,generation,state,
                           enumeration_complete,total_tasks,completed_tasks,failed_tasks";

fn row_run(row: &Row<'_>) -> rusqlite::Result<PersonAnalysisRun> {
    let state: String = row.get("state")?;
    // The store writes this spelling, so the store reads it back.
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
        folder_path: std::path::PathBuf::from(row.get::<_, String>("folder_path")?),
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

fn row_task(row: &Row<'_>) -> rusqlite::Result<PersonAnalysisTask> {
    Ok(PersonAnalysisTask {
        run_id: row.get("run_id")?,
        folder_path: std::path::PathBuf::from(row.get::<_, String>("folder_path")?),
        asset_path: std::path::PathBuf::from(row.get::<_, String>("asset_path")?),
        source_revision: row.get("source_revision")?,
        stage_id: row.get("stage_id")?,
        stage_fingerprint: row.get("stage_fingerprint")?,
        stage_order: row.get::<_, i64>("stage_order")? as u32,
        claim_token: String::new(),
    })
}

/// One detection row, as a detection stage writes it.
///
/// The boxes and the landmark set arrive as domain values; how they are spelled
/// in the column is the store's business, so the domain never sees the JSON.
pub struct NewDetection<'a> {
    pub folder_path: &'a str,
    pub asset_path: &'a str,
    pub instance_id: &'a str,
    pub source_revision: &'a str,
    pub producer_fingerprint: &'a str,
    pub pipeline_fingerprint: &'a str,
    pub run_id: &'a str,
    pub face_box: Option<[f64; 4]>,
    pub face_landmarks: Option<[[f32; 2]; 5]>,
    pub body_box: Option<[f64; 4]>,
    pub face_score: Option<f32>,
    pub body_score: Option<f32>,
    pub association_score: Option<f32>,
}

/// Drops the stage's previous detections for one asset.
pub fn clear_stage_detections(
    connection: &Connection,
    folder_path: &str,
    asset_path: &str,
    producer_fingerprint: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "DELETE FROM person_instances_cache WHERE folder_path=?1 AND asset_path=?2
           AND producer_fingerprint=?3",
        params![folder_path, asset_path, producer_fingerprint],
    )?;
    Ok(())
}

/// Writes one detection row.
pub fn insert_detection(
    connection: &Connection,
    detection: &NewDetection<'_>,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_instances_cache(folder_path,asset_path,instance_id,source_revision,
           producer_fingerprint,pipeline_fingerprint,run_id,face_box,face_landmarks,body_box,
           face_score,body_score,association_score)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![
            detection.folder_path,
            detection.asset_path,
            detection.instance_id,
            detection.source_revision,
            detection.producer_fingerprint,
            detection.pipeline_fingerprint,
            detection.run_id,
            detection
                .face_box
                .map(|value| serde_json::to_string(&value))
                .transpose()?,
            detection
                .face_landmarks
                .map(|value| serde_json::to_string(&value))
                .transpose()?,
            detection
                .body_box
                .map(|value| serde_json::to_string(&value))
                .transpose()?,
            detection.face_score,
            detection.body_score,
            detection.association_score
        ],
    )?;
    Ok(())
}

/// The detections one producer recorded for one asset at one source revision.
///
/// The caller's revision and producer are part of the query, so a rerun cannot
/// read the set it is about to replace.
pub fn list_detections(
    connection: &Connection,
    folder_path: &str,
    asset_path: &str,
    source_revision: &str,
    producer_fingerprint: &str,
) -> Result<Vec<DetectedPersonInstance>, StoreError> {
    let mut query = connection.prepare(
        "SELECT instance_id,face_box,face_landmarks,body_box,face_score,body_score,association_score
         FROM person_instances_cache WHERE folder_path=?1 AND asset_path=?2
           AND source_revision=?3 AND producer_fingerprint=?4 ORDER BY instance_id",
    )?;
    let rows = query.query_map(
        params![
            folder_path,
            asset_path,
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

/// One feature row, as an encoding stage writes it.
pub struct NewFeature<'a> {
    pub folder_path: &'a str,
    pub asset_path: &'a str,
    pub instance_id: &'a str,
    pub source_revision: &'a str,
    pub feature_space_id: &'a str,
    pub pipeline_fingerprint: &'a str,
    /// The normalized vector, little-endian `f32`.
    pub vector: &'a [u8],
}

/// Claims a feature space for a producer, keeping whichever contract is stored.
///
/// `INSERT OR IGNORE` rather than an upsert: a second producer claiming the
/// same id with a different dimension or producer has to be detected by the
/// caller reading the contract back, not silently overwritten here.
pub fn ensure_feature_space(
    connection: &Connection,
    feature_space_id: &str,
    modality: FeatureModality,
    dimension: usize,
    producer_fingerprint: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT OR IGNORE INTO person_feature_spaces(id,modality,dimension,producer_fingerprint)
         VALUES (?1,?2,?3,?4)",
        params![
            feature_space_id,
            modality.as_str(),
            dimension,
            producer_fingerprint
        ],
    )?;
    Ok(())
}

/// The modality, dimension, and producer a feature space was first stored with.
pub fn feature_space_contract(
    connection: &Connection,
    feature_space_id: &str,
) -> Result<Option<(String, i64, String)>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT modality,dimension,producer_fingerprint FROM person_feature_spaces WHERE id=?1",
            [feature_space_id],
            |row| {
                Ok((
                    row.get("modality")?,
                    row.get("dimension")?,
                    row.get("producer_fingerprint")?,
                ))
            },
        )
        .optional()?)
}

/// The stored dimension of a feature space, if it exists.
pub fn feature_space_dimension(
    connection: &Connection,
    feature_space_id: &str,
) -> Result<Option<i64>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT dimension FROM person_feature_spaces WHERE id=?1",
            [feature_space_id],
            |row| row.get("dimension"),
        )
        .optional()?)
}

/// Stores one vector, replacing the row for the same instance and space.
pub fn upsert_feature(connection: &Connection, feature: &NewFeature<'_>) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_features_cache(folder_path,asset_path,instance_id,source_revision,feature_space_id,pipeline_fingerprint,vector)
         VALUES (?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(folder_path,asset_path,instance_id,feature_space_id) DO UPDATE SET
           source_revision=excluded.source_revision,pipeline_fingerprint=excluded.pipeline_fingerprint,
           vector=excluded.vector,updated_at=unixepoch()",
        params![
            feature.folder_path,
            feature.asset_path,
            feature.instance_id,
            feature.source_revision,
            feature.feature_space_id,
            feature.pipeline_fingerprint,
            feature.vector
        ],
    )?;
    Ok(())
}

/// One page of an exhaustive threshold search over a folder's cached vectors.
///
/// The threshold applies to every eligible row before the page is counted, so
/// the scan and the page are one query: taking a top-K first would drop
/// candidates a large folder should have offered. A row whose asset no longer
/// carries the revision its vector was computed from is skipped before the
/// page counter advances — `current_sources` is the caller's snapshot, and
/// what makes a revision current is the caller's rule.
pub struct FeatureQuery<'a> {
    pub folder_path: &'a str,
    pub feature_space_id: &'a str,
    /// The normalized query vector, already serialized little-endian.
    pub vector: &'a [u8],
    pub min_similarity: f32,
    /// When set, only instances the current detection stage still records.
    pub detection_producer_fingerprint: Option<&'a str>,
    /// `(asset_path, source_revision)` for every asset in the current snapshot.
    pub current_sources: &'a [(String, String)],
    /// `(limit, offset)` over the rows that survive the staleness check.
    pub page: (usize, usize),
}

/// Scores every cached vector in the folder against the query, ordered.
pub fn search_features(
    connection: &Connection,
    query: &FeatureQuery<'_>,
) -> Result<Vec<PersonFeatureMatch>, StoreError> {
    let (limit, offset) = query.page;
    let mut statement = connection.prepare(
        "SELECT feature_row_id,asset_path,instance_id,source_revision,pipeline_fingerprint,
           1.0-vec_distance_cosine(vector,?1) AS similarity
         FROM person_features_cache AS f WHERE folder_path=?2 AND feature_space_id=?3
           AND 1.0-vec_distance_cosine(vector,?1)>=?4
           AND (?5 IS NULL OR EXISTS (
             SELECT 1 FROM person_instances_cache AS d
              WHERE d.folder_path=f.folder_path AND d.asset_path=f.asset_path
                AND d.instance_id=f.instance_id AND d.source_revision=f.source_revision
                AND d.producer_fingerprint=?5))
         ORDER BY similarity DESC,asset_path,instance_id,feature_row_id",
    )?;
    let rows = statement.query_map(
        params![
            query.vector,
            query.folder_path,
            query.feature_space_id,
            query.min_similarity,
            query.detection_producer_fingerprint
        ],
        |row| {
            Ok(PersonFeatureMatch {
                feature_row_id: row.get("feature_row_id")?,
                asset_path: std::path::PathBuf::from(row.get::<_, String>("asset_path")?),
                instance_id: row.get("instance_id")?,
                source_revision: row.get("source_revision")?,
                pipeline_fingerprint: row.get("pipeline_fingerprint")?,
                similarity: row.get("similarity")?,
            })
        },
    )?;
    let current: std::collections::HashMap<&str, &str> = query
        .current_sources
        .iter()
        .map(|(path, revision)| (path.as_str(), revision.as_str()))
        .collect();
    let mut matches = Vec::with_capacity(limit);
    let mut eligible_seen = 0usize;
    for row in rows {
        let candidate = row?;
        if current
            .get(candidate.asset_path.to_string_lossy().as_ref())
            .copied()
            != Some(candidate.source_revision.as_str())
        {
            continue;
        }
        if eligible_seen >= offset {
            matches.push(candidate);
            if matches.len() == limit {
                break;
            }
        }
        eligible_seen += 1;
    }
    Ok(matches)
}

/// The operation and run a request id already recorded, if any.
pub fn analysis_request(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<(String, String)>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT operation,run_id FROM person_analysis_requests WHERE request_id=?1",
            [request_id],
            |row| Ok((row.get("operation")?, row.get("run_id")?)),
        )
        .optional()?)
}

/// The run row itself, without judging whether it is still the head.
pub fn analysis_run(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<PersonAnalysisRun>, StoreError> {
    Ok(connection
        .query_row(
            &format!("SELECT {RUN_COLUMNS} FROM person_analysis_runs WHERE run_id=?1"),
            [run_id],
            row_run,
        )
        .optional()?)
}

/// Whether a run id exists at all, whatever its state.
pub fn analysis_run_exists(connection: &Connection, run_id: &str) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM person_analysis_runs WHERE run_id=?1) AS present",
        [run_id],
        |row| row.get("present"),
    )?)
}

/// The generation and run the folder's head currently points at.
pub fn analysis_head(
    connection: &Connection,
    folder_path: &str,
) -> Result<Option<(i64, String)>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT generation,run_id FROM person_analysis_heads WHERE folder_path=?1",
            [folder_path],
            |row| Ok((row.get("generation")?, row.get("run_id")?)),
        )
        .optional()?)
}

/// Whether the folder's head is this run at this generation.
pub fn analysis_head_matches(
    connection: &Connection,
    folder_path: &str,
    run_id: &str,
    generation: i64,
) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM person_analysis_heads
           WHERE folder_path=?1 AND run_id=?2 AND generation=?3) AS present",
        params![folder_path, run_id, generation],
        |row| row.get("present"),
    )?)
}

/// Whether the run is still the head, at its own generation, and unfinished.
pub fn analysis_is_current(connection: &Connection, run_id: &str) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM person_analysis_runs r
           JOIN person_analysis_heads h ON h.folder_path=r.folder_path
          WHERE r.run_id=?1 AND h.run_id=r.run_id AND h.generation=r.generation
            AND r.state IN ('queued','running')) AS present",
        [run_id],
        |row| row.get("present"),
    )?)
}

/// Cancels a run if it has not finished.
pub fn cancel_analysis_run(connection: &Connection, run_id: &str) -> Result<usize, StoreError> {
    Ok(connection.execute(
        "UPDATE person_analysis_runs SET state='cancelled',updated_at=unixepoch()
         WHERE run_id=?1 AND state IN ('queued','running')",
        [run_id],
    )?)
}

/// A fresh opaque id, as the runs and tasks tables use them.
pub fn new_cache_id(connection: &Connection) -> Result<String, StoreError> {
    Ok(
        connection.query_row("SELECT lower(hex(randomblob(16))) AS id", [], |row| {
            row.get("id")
        })?,
    )
}

pub struct NewAnalysisRun<'a> {
    pub run_id: &'a str,
    pub folder_path: &'a str,
    pub pipeline_id: &'a str,
    pub pipeline_fingerprint: &'a str,
    pub generation: i64,
}

/// Inserts a queued run and points the folder's head at it.
pub fn insert_analysis_run(
    connection: &Connection,
    run: &NewAnalysisRun<'_>,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_analysis_runs(run_id,folder_path,pipeline_id,pipeline_fingerprint,generation,state)
         VALUES (?1,?2,?3,?4,?5,'queued')",
        params![
            run.run_id,
            run.folder_path,
            run.pipeline_id,
            run.pipeline_fingerprint,
            run.generation
        ],
    )?;
    connection.execute(
        "INSERT INTO person_analysis_heads(folder_path,generation,run_id) VALUES (?1,?2,?3)
         ON CONFLICT(folder_path) DO UPDATE SET generation=excluded.generation,run_id=excluded.run_id",
        params![run.folder_path, run.generation, run.run_id],
    )?;
    Ok(())
}

/// Records an idempotency key for an analysis operation.
pub fn insert_analysis_request(
    connection: &Connection,
    request_id: &str,
    operation: &str,
    run_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_analysis_requests(request_id,operation,run_id) VALUES (?1,?2,?3)",
        params![request_id, operation, run_id],
    )?;
    Ok(())
}

/// The revision, fingerprint, and order a task was registered with.
pub fn analysis_task_identity(
    connection: &Connection,
    run_id: &str,
    asset_path: &str,
    stage_id: &str,
) -> Result<Option<(String, String, i64)>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT source_revision,stage_fingerprint,stage_order
             FROM person_analysis_tasks WHERE run_id=?1 AND asset_path=?2 AND stage_id=?3",
            params![run_id, asset_path, stage_id],
            |row| {
                Ok((
                    row.get("source_revision")?,
                    row.get("stage_fingerprint")?,
                    row.get("stage_order")?,
                ))
            },
        )
        .optional()?)
}

/// The stored record of a task, as the commit path fences against it:
/// `(source_revision, stage_fingerprint, stage_order, state, claim_token)`.
///
/// A tuple rather than a struct because the only thing a caller does with it is
/// compare it to the values it is about to commit.
pub type AnalysisTaskRecord = (String, String, i64, String, Option<String>);

/// The full stored record of a task, as the commit path fences against it.
pub fn analysis_task_record(
    connection: &Connection,
    run_id: &str,
    asset_path: &str,
    stage_id: &str,
) -> Result<Option<AnalysisTaskRecord>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT source_revision,stage_fingerprint,stage_order,state,claim_token
             FROM person_analysis_tasks WHERE run_id=?1 AND asset_path=?2 AND stage_id=?3",
            params![run_id, asset_path, stage_id],
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
        .optional()?)
}

/// Whether another stage already holds this position in the asset's order.
pub fn analysis_task_order_taken(
    connection: &Connection,
    run_id: &str,
    asset_path: &str,
    stage_order: i64,
) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM person_analysis_tasks
           WHERE run_id=?1 AND asset_path=?2 AND stage_order=?3) AS present",
        params![run_id, asset_path, stage_order],
        |row| row.get("present"),
    )?)
}

pub struct NewAnalysisTask<'a> {
    pub run_id: &'a str,
    pub asset_path: &'a str,
    pub source_revision: &'a str,
    pub stage_id: &'a str,
    pub stage_fingerprint: &'a str,
    pub stage_order: i64,
}

/// Registers one queued task.
pub fn insert_analysis_task(
    connection: &Connection,
    task: &NewAnalysisTask<'_>,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_analysis_tasks(run_id,asset_path,source_revision,
            stage_id,stage_fingerprint,stage_order,state)
         VALUES (?1,?2,?3,?4,?5,?6,'queued')",
        params![
            task.run_id,
            task.asset_path,
            task.source_revision,
            task.stage_id,
            task.stage_fingerprint,
            task.stage_order
        ],
    )?;
    Ok(())
}

/// Adds to the run's registered task count.
pub fn add_run_tasks(connection: &Connection, run_id: &str, added: i64) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE person_analysis_runs SET total_tasks=total_tasks+?2,updated_at=unixepoch()
         WHERE run_id=?1",
        params![run_id, added],
    )?;
    Ok(())
}

/// Marks enumeration finished and completes the run if nothing is outstanding.
pub fn seal_analysis_run(connection: &Connection, run_id: &str) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE person_analysis_runs SET enumeration_complete=1,
           state=CASE WHEN completed_tasks+failed_tasks=total_tasks
             THEN CASE WHEN failed_tasks=0 THEN 'completed' ELSE 'failed' END
             ELSE state END,
           updated_at=unixepoch() WHERE run_id=?1",
        [run_id],
    )?;
    Ok(())
}

/// The next runnable stage: queued, and no earlier stage of that asset open.
pub fn claimable_task(
    connection: &Connection,
    run_id: &str,
    stage_id: Option<&str>,
    asset_path: Option<&str>,
) -> Result<Option<PersonAnalysisTask>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT t.run_id,r.folder_path,t.asset_path,t.source_revision,t.stage_id,
                    t.stage_fingerprint,t.stage_order
             FROM person_analysis_tasks t JOIN person_analysis_runs r ON r.run_id=t.run_id
             WHERE t.run_id=?1 AND t.state='queued'
               AND (?2 IS NULL OR t.stage_id=?2) AND (?3 IS NULL OR t.asset_path=?3)
               AND NOT EXISTS (SELECT 1 FROM person_analysis_tasks earlier
                 WHERE earlier.run_id=t.run_id AND earlier.asset_path=t.asset_path
                   AND earlier.stage_order<t.stage_order AND earlier.state!='completed')
             ORDER BY t.asset_path,t.stage_order LIMIT 1",
            params![run_id, stage_id, asset_path],
            row_task,
        )
        .optional()?)
}

/// Attaches a claim token, but only if the task is still queued.
pub fn claim_analysis_task(
    connection: &Connection,
    run_id: &str,
    asset_path: &str,
    stage_id: &str,
    claim_token: &str,
) -> Result<usize, StoreError> {
    Ok(connection.execute(
        "UPDATE person_analysis_tasks SET state='running',claim_token=?4
         WHERE run_id=?1 AND asset_path=?2 AND stage_id=?3 AND state='queued'",
        params![run_id, asset_path, stage_id, claim_token],
    )?)
}

/// Moves a queued run to running when its first task is claimed.
pub fn start_analysis_run(connection: &Connection, run_id: &str) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE person_analysis_runs SET state='running',updated_at=unixepoch()
         WHERE run_id=?1 AND state='queued'",
        [run_id],
    )?;
    Ok(())
}

/// Requeues the claims an interrupted worker left behind.
pub fn requeue_running_tasks(connection: &Connection, run_id: &str) -> Result<usize, StoreError> {
    Ok(connection.execute(
        "UPDATE person_analysis_tasks SET state='queued',claim_token=NULL,error=NULL
         WHERE run_id=?1 AND state='running'",
        [run_id],
    )?)
}

/// Returns a run to queued after its claims were requeued.
pub fn requeue_analysis_run(connection: &Connection, run_id: &str) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE person_analysis_runs SET state='queued',updated_at=unixepoch()
         WHERE run_id=?1",
        [run_id],
    )?;
    Ok(())
}

/// Marks one task completed.
pub fn complete_analysis_task(
    connection: &Connection,
    run_id: &str,
    asset_path: &str,
    stage_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE person_analysis_tasks SET state='completed',error=NULL
         WHERE run_id=?1 AND asset_path=?2 AND stage_id=?3",
        params![run_id, asset_path, stage_id],
    )?;
    Ok(())
}

/// Counts one completion, finishing the run when the last task lands.
pub fn bump_completed_tasks(connection: &Connection, run_id: &str) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE person_analysis_runs SET completed_tasks=completed_tasks+1,
           state=CASE WHEN enumeration_complete=1 AND completed_tasks+failed_tasks+1=total_tasks
             THEN CASE WHEN failed_tasks=0 THEN 'completed' ELSE 'failed' END
             ELSE state END, updated_at=unixepoch() WHERE run_id=?1",
        [run_id],
    )?;
    Ok(())
}

/// Fails a claimed task, refusing if the claim no longer matches the row.
pub fn fail_analysis_task(
    connection: &Connection,
    task: &PersonAnalysisTask,
    asset_path: &str,
    error: &str,
) -> Result<usize, StoreError> {
    Ok(connection.execute(
        "UPDATE person_analysis_tasks SET state='failed',error=?4
         WHERE run_id=?1 AND asset_path=?2 AND stage_id=?3
           AND source_revision=?5 AND stage_fingerprint=?6 AND stage_order=?7
           AND claim_token=?8 AND state='running'",
        params![
            task.run_id,
            asset_path,
            task.stage_id,
            error,
            task.source_revision,
            task.stage_fingerprint,
            task.stage_order,
            task.claim_token
        ],
    )?)
}

/// Fails every later stage of the same asset that is still queued.
pub fn fail_dependent_tasks(
    connection: &Connection,
    run_id: &str,
    asset_path: &str,
    stage_order: i64,
) -> Result<usize, StoreError> {
    Ok(connection.execute(
        "UPDATE person_analysis_tasks SET state='failed',error='earlier stage failed'
         WHERE run_id=?1 AND asset_path=?2 AND stage_order>?3 AND state='queued'",
        params![run_id, asset_path, stage_order],
    )?)
}

/// Counts failures, finishing the run as failed when they cover every task.
pub fn bump_failed_tasks(
    connection: &Connection,
    run_id: &str,
    failed: i64,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE person_analysis_runs SET failed_tasks=failed_tasks+?2,
           state=CASE WHEN enumeration_complete=1
             AND completed_tasks+failed_tasks+?2=total_tasks THEN 'failed' ELSE state END,
           updated_at=unixepoch() WHERE run_id=?1",
        params![run_id, failed],
    )?;
    Ok(())
}
