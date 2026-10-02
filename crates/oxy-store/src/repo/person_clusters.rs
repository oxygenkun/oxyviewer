//! Statements for folder-level derived groups. No transactions or identity policy.
use crate::StoreError;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, PartialEq)]
pub struct ClusterEvidence {
    pub asset_path: String,
    pub instance_id: String,
    pub source_revision: String,
    pub face_box: String,
    pub face_score: f64,
    pub vector: Option<Vec<u8>>,
}

pub fn evidence(
    connection: &Connection,
    folder: &str,
    run: &str,
    detector: &str,
    space: &str,
    encoder: &str,
) -> Result<Vec<ClusterEvidence>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT d.asset_path,d.instance_id,d.source_revision,d.face_box,d.face_score,f.vector
         FROM person_instances_cache d
         LEFT JOIN person_features_cache f ON f.folder_path=d.folder_path
           AND f.asset_path=d.asset_path AND f.instance_id=d.instance_id
           AND f.source_revision=d.source_revision AND f.feature_space_id=?4
           AND EXISTS (SELECT 1 FROM person_feature_spaces s WHERE s.id=f.feature_space_id
             AND s.producer_fingerprint=?5 AND s.modality='face' AND s.dimension=512)
         WHERE d.folder_path=?1 AND d.run_id=?2 AND d.producer_fingerprint=?3
           AND d.face_box IS NOT NULL
         ORDER BY d.asset_path,d.instance_id",
    )?;
    Ok(statement
        .query_map(params![folder, run, detector, space, encoder], |row| {
            Ok(ClusterEvidence {
                asset_path: row.get("asset_path")?,
                instance_id: row.get("instance_id")?,
                source_revision: row.get("source_revision")?,
                face_box: row.get("face_box")?,
                face_score: row.get::<_, Option<f64>>("face_score")?.unwrap_or(0.0),
                vector: row.get("vector")?,
            })
        })?
        .collect::<Result<_, _>>()?)
}

pub fn snapshot(connection: &Connection, folder: &str) -> Result<Option<String>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT c.snapshot_json FROM person_cluster_snapshots c
         JOIN person_analysis_heads h ON h.folder_path=c.folder_path AND h.run_id=c.run_id
         JOIN person_analysis_runs r ON r.run_id=h.run_id AND r.state='completed'
         WHERE c.folder_path=?1",
            [folder],
            |row| row.get("snapshot_json"),
        )
        .optional()?)
}

pub fn save_snapshot(
    connection: &Connection,
    folder: &str,
    run: &str,
    json: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_cluster_snapshots(folder_path,run_id,snapshot_json) VALUES (?1,?2,?3)
         ON CONFLICT(folder_path) DO UPDATE SET run_id=excluded.run_id,snapshot_json=excluded.snapshot_json",
        params![folder, run, json],
    )?;
    Ok(())
}

pub fn completed_sources(
    connection: &Connection,
    run: &str,
    detector: &str,
) -> Result<Vec<(String, String)>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT asset_path,source_revision FROM person_analysis_tasks
         WHERE run_id=?1 AND stage_id=?2 AND state='completed' ORDER BY asset_path",
    )?;
    Ok(statement
        .query_map(params![run, detector], |row| {
            Ok((row.get("asset_path")?, row.get("source_revision")?))
        })?
        .collect::<Result<_, _>>()?)
}
