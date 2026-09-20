//! Face and person persistence.
//!
//! Two very different kinds of data share this module, and the difference is
//! load-bearing:
//!
//! * **Machine output** — [`FaceObservation`]s, embeddings, candidates, and
//!   clusters. Rebuildable at any time from the pixels plus the models. A new
//!   detector or embedder may invalidate all of it.
//! * **User facts** — persons and [`FaceDecision`]s. Never derived, never
//!   overwritten by an analyzer, and expected to outlive any number of model
//!   upgrades.
//!
//! Two invariants keep that split honest:
//!
//! 1. Replacing an asset's detections never deletes a decision. Decisions are
//!    bound to a normalized region, and a re-analysis re-binds them to the
//!    overlapping new observation. That is why confirming Alice survives a
//!    detector whose boxes moved a few pixels.
//! 2. A stored decision always wins over a machine proposal when the review
//!    queue is built, so a rejected candidate is never offered again.
//!
//! Geometry and similarity are stored as integer micro-units
//! (`value * 1_000_000`) to follow this crate's no-`REAL`-columns convention
//! while keeping exact round-tripping and cheap overlap comparisons.

use std::path::{Path, PathBuf};

use oxy_domain::{
    AssetId, AssetKind, CustomTagId, DecisionRecord, FACE_REVISION_VERSION, FaceAnalyzerSettings,
    FaceCandidate, FaceCluster, FaceDecision, FaceDecisionRecord, FaceLibraryStats,
    FaceObservation, FaceObservationId, FaceReviewFilter, FaceReviewItem, FaceReviewPage,
    FaceReviewState, NormalizedPoint, NormalizedRect, Person, PersonRecord, PixelSize,
    face_source_revision,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::Library;
use crate::LibraryError;

/// Minimum overlap for a re-analysis to re-bind a user decision to a new
/// detection. Detector upgrades move a box by a few pixels; a different face
/// overlaps far less.
pub const DECISION_REBIND_IOU: f32 = 0.5;

/// Bump when Full crop geometry, encoding, or orientation policy changes.
const FACE_CROP_POLICY_VERSION: &str = "full-crop-v1";

/// A detected face plus its embedding, as produced by the analyzer.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredFace {
    pub observation: FaceObservation,
    pub embedding: Vec<f32>,
    pub face_pixels: u32,
    pub clarity: f32,
}

/// Rebuildable JPEG cut from the same Full pixels as an observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFaceCrop {
    pub observation_id: FaceObservationId,
    pub size: u32,
    pub jpeg: Vec<u8>,
}

/// One atomic publication, optionally guarded by a projection admission revision.
pub struct FaceResultWrite<'a> {
    pub asset_path: &'a Path,
    pub asset_id: &'a str,
    pub source_revision: &'a str,
    pub detector_fingerprint: &'a str,
    pub embedder_fingerprint: &'a str,
    pub faces: &'a [StoredFace],
    pub crops: &'a [StoredFaceCrop],
    pub valid_at: Option<u64>,
    pub cancellation: Option<&'a oxy_runtime::CancellationToken>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct FaceRunCheckpoint {
    pub generation: u64,
    pub cursor: Option<PathBuf>,
}

/// One asset the analyzer still has to visit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceAnalysisTarget {
    pub path: PathBuf,
    pub kind: AssetKind,
    /// Revision of the source bytes this target was enumerated from.
    pub source_revision: String,
    pub modified_at_ms: u64,
    pub size_bytes: u64,
}

/// A durable user decision, including ones whose observation was replaced.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredDecision {
    pub decision_id: i64,
    pub observation_id: Option<FaceObservationId>,
    pub asset_id: AssetId,
    pub asset_path: PathBuf,
    pub region: NormalizedRect,
    pub decision: FaceDecision,
}

/// Adds the proposed-similarity column to a database created before it existed.
fn ensure_proposed_similarity_column(connection: &Connection) -> Result<(), rusqlite::Error> {
    let exists = connection
        .prepare("PRAGMA table_info(face_decisions)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == "proposed_similarity_micro");
    if !exists {
        connection.execute(
            "ALTER TABLE face_decisions ADD COLUMN proposed_similarity_micro INTEGER",
            [],
        )?;
    }
    Ok(())
}

fn ensure_face_quality_columns(connection: &Connection) -> Result<(), rusqlite::Error> {
    let columns = connection
        .prepare("PRAGMA table_info(face_observations)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|column| column == "face_pixels") {
        connection.execute(
            "ALTER TABLE face_observations ADD COLUMN face_pixels INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !columns.iter().any(|column| column == "clarity_score_micro") {
        connection.execute(
            "ALTER TABLE face_observations ADD COLUMN clarity_score_micro INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

fn ensure_crop_policy_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    let transaction = connection.unchecked_transaction()?;
    let has_policy = transaction
        .prepare("PRAGMA table_info(face_crops)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == "policy_version");
    if !has_policy {
        // Legacy crops have no verifiable rendering policy. Rebuild only this
        // disposable cache, including its new composite primary key.
        transaction.execute_batch(
            "DROP TABLE face_crops;
             CREATE TABLE face_crops (
                 observation_id TEXT NOT NULL
                     REFERENCES face_observations(observation_id) ON DELETE CASCADE,
                 size INTEGER NOT NULL,
                 policy_version TEXT NOT NULL,
                 jpeg BLOB NOT NULL,
                 PRIMARY KEY (observation_id, size, policy_version)
             );",
        )?;
    }
    transaction.commit()
}

pub(super) fn ensure_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS indexed_assets_face_cursor ON indexed_assets(path, root_path);
        CREATE TABLE IF NOT EXISTS face_runtime_state (id INTEGER PRIMARY KEY CHECK(id=1), index_generation INTEGER NOT NULL);
        INSERT OR IGNORE INTO face_runtime_state VALUES(1, 0);
        CREATE TABLE IF NOT EXISTS analyzer_schema_owners (analyzer_id TEXT PRIMARY KEY, schema_version INTEGER NOT NULL);
        INSERT OR REPLACE INTO analyzer_schema_owners VALUES('faces', 1);
        CREATE TABLE IF NOT EXISTS face_tag_assignments (asset_path TEXT NOT NULL, tag_id INTEGER NOT NULL REFERENCES custom_tags(id) ON DELETE CASCADE, owned INTEGER NOT NULL, PRIMARY KEY(asset_path, tag_id));
        CREATE TABLE IF NOT EXISTS face_runs (run_key TEXT PRIMARY KEY, generation INTEGER NOT NULL, cursor TEXT);
        CREATE TRIGGER IF NOT EXISTS face_index_completed AFTER INSERT ON indexed_roots BEGIN UPDATE face_runtime_state SET index_generation=index_generation+1; END;
        CREATE TRIGGER IF NOT EXISTS face_index_updated AFTER UPDATE ON indexed_roots BEGIN UPDATE face_runtime_state SET index_generation=index_generation+1; END;
        CREATE TRIGGER IF NOT EXISTS face_index_removed AFTER DELETE ON indexed_roots BEGIN UPDATE face_runtime_state SET index_generation=index_generation+1; END;
        CREATE TABLE IF NOT EXISTS face_observations (
            observation_id TEXT PRIMARY KEY NOT NULL,
            asset_path TEXT NOT NULL,
            asset_id TEXT NOT NULL,
            source_revision TEXT NOT NULL,
            detector_fingerprint TEXT NOT NULL,
            local_index INTEGER NOT NULL,
            x_micro INTEGER NOT NULL,
            y_micro INTEGER NOT NULL,
            width_micro INTEGER NOT NULL,
            height_micro INTEGER NOT NULL,
            detection_score_micro INTEGER NOT NULL,
            face_pixels INTEGER NOT NULL DEFAULT 0,
            clarity_score_micro INTEGER NOT NULL DEFAULT 0,
            landmarks_json TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS face_observations_asset
            ON face_observations(asset_path);
        CREATE INDEX IF NOT EXISTS face_observations_fingerprint
            ON face_observations(detector_fingerprint, asset_path);

        CREATE TABLE IF NOT EXISTS face_embeddings (
            observation_id TEXT NOT NULL
                REFERENCES face_observations(observation_id) ON DELETE CASCADE,
            embedder_fingerprint TEXT NOT NULL,
            dimensions INTEGER NOT NULL,
            vector BLOB NOT NULL,
            PRIMARY KEY (observation_id, embedder_fingerprint)
        );

        CREATE TABLE IF NOT EXISTS face_crops (
            observation_id TEXT NOT NULL
                REFERENCES face_observations(observation_id) ON DELETE CASCADE,
            size INTEGER NOT NULL,
            policy_version TEXT NOT NULL,
            jpeg BLOB NOT NULL,
            PRIMARY KEY (observation_id, size, policy_version)
        );

        CREATE TABLE IF NOT EXISTS face_asset_scans (
            asset_path TEXT PRIMARY KEY NOT NULL,
            asset_id TEXT NOT NULL,
            source_revision TEXT NOT NULL,
            detector_fingerprint TEXT NOT NULL,
            face_count INTEGER NOT NULL,
            analyzed_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS face_asset_scans_fingerprint
            ON face_asset_scans(detector_fingerprint);

        CREATE TABLE IF NOT EXISTS face_candidates (
            observation_id TEXT NOT NULL
                REFERENCES face_observations(observation_id) ON DELETE CASCADE,
            person_id TEXT NOT NULL,
            matcher_fingerprint TEXT NOT NULL,
            similarity_micro INTEGER NOT NULL,
            state TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY (observation_id, person_id, matcher_fingerprint)
        );
        CREATE INDEX IF NOT EXISTS face_candidates_person
            ON face_candidates(person_id);

        CREATE TABLE IF NOT EXISTS face_clusters (
            cluster_id TEXT NOT NULL,
            clustering_fingerprint TEXT NOT NULL,
            representative_observation_id TEXT NOT NULL,
            member_count INTEGER NOT NULL,
            cohesion_micro INTEGER NOT NULL,
            outlier_ids_json TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY (cluster_id, clustering_fingerprint)
        );

        CREATE TABLE IF NOT EXISTS face_cluster_members (
            cluster_id TEXT NOT NULL,
            clustering_fingerprint TEXT NOT NULL,
            observation_id TEXT NOT NULL,
            PRIMARY KEY (cluster_id, clustering_fingerprint, observation_id)
        );

        CREATE TABLE IF NOT EXISTS persons (
            person_id TEXT PRIMARY KEY NOT NULL,
            display_name TEXT NOT NULL,
            name_key TEXT NOT NULL,
            linked_tag_id INTEGER REFERENCES custom_tags(id) ON DELETE SET NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS persons_name_key ON persons(name_key);

        CREATE TABLE IF NOT EXISTS face_decisions (
            decision_id INTEGER PRIMARY KEY AUTOINCREMENT,
            observation_id TEXT,
            asset_path TEXT NOT NULL,
            asset_id TEXT NOT NULL,
            decision_kind TEXT NOT NULL,
            person_id TEXT,
            x_micro INTEGER NOT NULL,
            y_micro INTEGER NOT NULL,
            width_micro INTEGER NOT NULL,
            height_micro INTEGER NOT NULL,
            proposed_similarity_micro INTEGER,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS face_decisions_asset
            ON face_decisions(asset_path);
        CREATE INDEX IF NOT EXISTS face_decisions_person
            ON face_decisions(person_id);
        CREATE INDEX IF NOT EXISTS face_decisions_observation
            ON face_decisions(observation_id);

        CREATE TABLE IF NOT EXISTS face_decision_events (
            event_id INTEGER PRIMARY KEY AUTOINCREMENT,
            decision_id INTEGER,
            observation_id TEXT,
            decision_kind TEXT NOT NULL,
            person_id TEXT,
            x_micro INTEGER NOT NULL DEFAULT 0,
            y_micro INTEGER NOT NULL DEFAULT 0,
            width_micro INTEGER NOT NULL DEFAULT 0,
            height_micro INTEGER NOT NULL DEFAULT 0,
            payload_json TEXT NOT NULL DEFAULT '{}',
            created_at INTEGER NOT NULL DEFAULT (unixepoch())
        );

        CREATE TABLE IF NOT EXISTS face_settings (
            id INTEGER PRIMARY KEY CHECK(id = 1),
            settings_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );",
    )?;
    ensure_proposed_similarity_column(connection)?;
    ensure_face_quality_columns(connection)?;
    ensure_crop_policy_schema(connection)
}

fn to_micro(value: f32) -> i64 {
    (f64::from(value) * 1_000_000.0).round() as i64
}

fn from_micro(value: i64) -> f32 {
    (value as f64 / 1_000_000.0) as f32
}

fn rect_columns(rect: NormalizedRect) -> (i64, i64, i64, i64) {
    (
        to_micro(rect.x),
        to_micro(rect.y),
        to_micro(rect.width),
        to_micro(rect.height),
    )
}

fn rect_from_row(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> Result<NormalizedRect, rusqlite::Error> {
    Ok(NormalizedRect::new(
        from_micro(row.get(offset)?),
        from_micro(row.get(offset + 1)?),
        from_micro(row.get(offset + 2)?),
        from_micro(row.get(offset + 3)?),
    ))
}

fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(embedding.len() * 4);
    for value in embedding {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn landmarks_to_json(landmarks: &[NormalizedPoint]) -> String {
    let values: Vec<[f32; 2]> = landmarks.iter().map(|point| [point.x, point.y]).collect();
    serde_json::to_string(&values).unwrap_or_else(|_| "[]".into())
}

fn landmarks_from_json(value: &str) -> Vec<NormalizedPoint> {
    serde_json::from_str::<Vec<[f32; 2]>>(value)
        .unwrap_or_default()
        .into_iter()
        .map(|point| NormalizedPoint::new(point[0], point[1]))
        .collect()
}

fn parse_decision(kind: &str, person_id: Option<String>) -> Option<FaceDecision> {
    match kind {
        "confirm_person" => Some(FaceDecision::ConfirmPerson {
            person_id: person_id?,
        }),
        "reject_person" => Some(FaceDecision::RejectPerson {
            person_id: person_id?,
        }),
        "not_face" => Some(FaceDecision::NotFace),
        _ => None,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

/// SQL predicate implementing [`FaceReviewFilter`]. Kept in SQL so paging does
/// not count rows the caller will not see.
fn filter_predicate(filter: FaceReviewFilter) -> &'static str {
    match filter {
        FaceReviewFilter::All => "1 = 1",
        FaceReviewFilter::Pending => {
            "d.decision_id IS NULL AND EXISTS (
                SELECT 1 FROM face_candidates c
                WHERE c.observation_id = o.observation_id AND c.state = 'pending')"
        }
        FaceReviewFilter::Unknown => {
            "d.decision_id IS NULL AND NOT EXISTS (
                SELECT 1 FROM face_candidates c
                WHERE c.observation_id = o.observation_id AND c.state = 'pending')"
        }
        // The whole queue a reviewer still owes an answer for, whether or not
        // the matcher had an opinion.
        FaceReviewFilter::Unreviewed => "d.decision_id IS NULL",
        FaceReviewFilter::Confirmed => "d.decision_kind = 'confirm_person'",
        FaceReviewFilter::Rejected => "d.decision_kind IN ('reject_person', 'not_face')",
    }
}

/// Bounded page of indexed assets; source identity is verified by the host.
impl Library {
    /// A changed index starts a fresh pass; per-asset checkpoints survive it.
    pub fn resume_face_run(&self, key: &str) -> Result<FaceRunCheckpoint, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let generation: u64 = transaction.query_row(
            "SELECT index_generation FROM face_runtime_state WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        transaction.execute("INSERT INTO face_runs(run_key, generation) VALUES (?1, ?2) ON CONFLICT(run_key) DO UPDATE SET cursor=CASE WHEN generation=excluded.generation THEN cursor ELSE NULL END, generation=excluded.generation", params![key, generation])?;
        let cursor: Option<String> = transaction.query_row(
            "SELECT cursor FROM face_runs WHERE run_key=?1",
            params![key],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(FaceRunCheckpoint {
            generation,
            cursor: cursor.map(PathBuf::from),
        })
    }

    pub fn checkpoint_face_run(
        &self,
        key: &str,
        generation: u64,
        cursor: &Path,
    ) -> Result<(), LibraryError> {
        self.connection.lock().execute(
            "UPDATE face_runs SET cursor=?1 WHERE run_key=?2 AND generation=?3",
            params![cursor.to_string_lossy(), key, generation],
        )?;
        Ok(())
    }

    /// Returns false when indexing changed and the caller must visit a fresh pass.
    pub fn finish_face_run(&self, key: &str, generation: u64) -> Result<bool, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let current: u64 = transaction.query_row(
            "SELECT index_generation FROM face_runtime_state WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        if current != generation {
            return Ok(false);
        }
        transaction.execute(
            "DELETE FROM face_runs WHERE run_key=?1 AND generation=?2",
            params![key, generation],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn face_analysis_page(
        &self,
        request: &oxy_domain::FaceAnalysisRequest,
        after: Option<&Path>,
    ) -> Result<Vec<FaceAnalysisTarget>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let paths = serde_json::to_string(&request.paths)?;
        let root = request
            .root_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        let directory = request
            .directory
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        let after = after
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut statement = connection.prepare(
            "SELECT a.path, a.kind, a.modified_at_ms, a.size_bytes
             FROM indexed_assets a JOIN library_roots r ON r.path = a.root_path
             WHERE a.path > ?1
               AND (?2 IS NULL OR a.root_path = ?2)
               AND (?3 IS NULL OR a.parent_path = ?3)
               AND (?4 = '[]' OR a.path IN (SELECT value FROM json_each(?4)))
             GROUP BY a.path ORDER BY a.path LIMIT 256",
        )?;
        let rows = statement.query_map(params![after, root, directory, paths], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, u64>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (path, kind, modified_at_ms, size_bytes) = row?;
            Ok(FaceAnalysisTarget {
                path: PathBuf::from(path),
                kind: super::parse_kind(&kind)?,
                source_revision: String::new(),
                modified_at_ms,
                size_bytes,
            })
        })
        .collect()
    }

    pub fn begin_face_analysis(
        &self,
        path: &Path,
        source_revision: &str,
    ) -> Result<u64, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let revision = super::next_resource_revision(&transaction)?;
        transaction.execute(
            "INSERT INTO resource_projections(path, parent_path, projection_kind, source_revision, valid_at, state_revision, status)
             VALUES (?1, ?2, 'faces:v1', ?3, ?4, ?4, 'pending')
             ON CONFLICT(path, projection_kind) DO UPDATE SET source_revision=excluded.source_revision, valid_at=excluded.valid_at, state_revision=excluded.state_revision, status='pending', result_json=NULL, error=NULL",
            params![path.to_string_lossy(), path.parent().unwrap_or(Path::new("")).to_string_lossy(), source_revision, revision],
        )?;
        transaction.commit()?;
        Ok(revision)
    }

    pub fn fail_face_analysis(
        &self,
        path: &Path,
        valid_at: u64,
        code: &str,
    ) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let revision = super::next_resource_revision(&transaction)?;
        transaction.execute("UPDATE resource_projections SET status='error', error=?1, state_revision=?2 WHERE path=?3 AND projection_kind='faces:v1' AND valid_at=?4 AND status='pending'", params![code, revision, path.to_string_lossy(), valid_at])?;
        transaction.commit()?;
        Ok(())
    }

    /// Clear machine data while retaining user facts and revision tombstones.
    pub fn clear_face_cache(&self) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let revision = super::next_resource_revision(&transaction)?;
        transaction.execute("UPDATE resource_projections SET status='error', error='cleared', result_json=NULL, valid_at=?1, state_revision=?1 WHERE projection_kind='faces:v1'", params![revision])?;
        transaction.execute_batch("DELETE FROM face_candidates; DELETE FROM face_cluster_members; DELETE FROM face_clusters; DELETE FROM face_crops; DELETE FROM face_embeddings; UPDATE face_decisions SET observation_id=NULL; DELETE FROM face_observations; DELETE FROM face_asset_scans; DELETE FROM face_runs;")?;
        transaction.commit()?;
        Ok(())
    }

    pub fn invalidate_face_projections(&self) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let revision = super::next_resource_revision(&transaction)?;
        transaction.execute("UPDATE resource_projections SET status='error', error='invalidated', state_revision=?1, valid_at=?1 WHERE projection_kind='faces:v1'", params![revision])?;
        transaction.commit()?;
        Ok(())
    }

    /// Checkpoint lookup uses the live source identity, never the cheap index stat.
    pub fn face_scan_is_current(
        &self,
        path: &Path,
        revision: &str,
        detector: &str,
    ) -> Result<bool, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM face_asset_scans s JOIN resource_projections p ON p.path=s.asset_path AND p.projection_kind='faces:v1' WHERE s.asset_path = ?1 AND s.source_revision = ?2 AND s.detector_fingerprint = ?3 AND p.source_revision=?2 AND p.status='ready')",
            params![path.to_string_lossy(), revision, detector], |row| row.get(0),
        )?)
    }
}

impl Library {
    /// Persists scan-time crops in the rebuildable face cache. Old crops are
    /// removed by the observation foreign key when an asset is re-analyzed.
    pub fn store_face_crops(&self, crops: &[StoredFaceCrop]) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT OR REPLACE INTO face_crops (observation_id, size, policy_version, jpeg)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for crop in crops {
                statement.execute(params![
                    crop.observation_id,
                    crop.size,
                    FACE_CROP_POLICY_VERSION,
                    crop.jpeg
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Both detection and embedding identity must match the ready projection.
    pub fn face_models_are_current(
        &self,
        path: &Path,
        revision: &str,
        detector: &str,
        embedder: &str,
    ) -> Result<bool, LibraryError> {
        let connection = self.read_connection();
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM face_asset_scans s JOIN resource_projections p ON p.path=s.asset_path AND p.projection_kind='faces:v1' WHERE s.asset_path=?1 AND s.source_revision=?2 AND s.detector_fingerprint=?3 AND p.source_revision=?2 AND p.status='ready' AND json_extract(p.result_json, '$.embedderFingerprint')=?4)",
            params![path.to_string_lossy(), revision, detector, embedder], |row| row.get(0),
        )?)
    }

    /// Returns the smallest cached crop large enough for the requested UI size.
    pub fn face_crop(
        &self,
        observation_id: &str,
        size: u32,
    ) -> Result<Option<(u32, Vec<u8>)>, LibraryError> {
        let connection = self.connection.lock();
        connection
            .query_row(
                "SELECT size, jpeg FROM face_crops
                 WHERE observation_id = ?1 AND size >= ?2 AND policy_version = ?3
                 ORDER BY size ASC LIMIT 1",
                params![observation_id, size, FACE_CROP_POLICY_VERSION],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Into::into)
    }

    /// Replaces every detection for one asset and re-binds surviving user
    /// decisions to the overlapping new detections.
    pub fn replace_asset_faces(
        &self,
        asset_path: &Path,
        asset_id: &str,
        source_revision: &str,
        detector_fingerprint: &str,
        embedder_fingerprint: &str,
        faces: &[StoredFace],
    ) -> Result<(), LibraryError> {
        self.accept_face_result(FaceResultWrite {
            asset_path,
            asset_id,
            source_revision,
            detector_fingerprint,
            embedder_fingerprint,
            faces,
            crops: &[],
            valid_at: None,
            cancellation: None,
        })
    }

    /// Only the still-current request may publish derived rows and its projection.
    pub fn accept_face_result(&self, result: FaceResultWrite<'_>) -> Result<(), LibraryError> {
        let FaceResultWrite {
            asset_path,
            asset_id,
            source_revision,
            detector_fingerprint,
            embedder_fingerprint,
            faces,
            crops,
            valid_at,
            cancellation,
        } = result;
        let asset_path_string = asset_path.to_string_lossy().to_string();
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;

        if cancellation.is_some_and(oxy_runtime::CancellationToken::is_cancelled) {
            return Err(LibraryError::StaleFaceResult);
        }
        if let Some(expected) = valid_at {
            let valid: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM resource_projections WHERE path = ?1 AND projection_kind = 'faces:v1' AND source_revision = ?2 AND valid_at = ?3 AND status = 'pending')",
                params![asset_path_string, source_revision, expected], |row| row.get(0),
            )?;
            if !valid {
                return Err(LibraryError::StaleFaceResult);
            }
        }
        let decisions = decisions_for_asset(&transaction, &asset_path_string)?;

        transaction.execute(
            "DELETE FROM face_observations WHERE asset_path = ?1",
            params![asset_path_string],
        )?;

        let mut inserted: Vec<(FaceObservationId, NormalizedRect)> = Vec::new();
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO face_observations (
                    observation_id, asset_path, asset_id, source_revision,
                    detector_fingerprint, local_index, x_micro, y_micro,
                    width_micro, height_micro, detection_score_micro,
                    face_pixels, clarity_score_micro, landmarks_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            )?;
            let mut embedding_statement = transaction.prepare_cached(
                "INSERT INTO face_embeddings (
                    observation_id, embedder_fingerprint, dimensions, vector
                 ) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for face in faces {
                let observation = &face.observation;
                let Some(bbox) = observation.bbox.clamp_unit() else {
                    continue;
                };
                let (x, y, width, height) = rect_columns(bbox);
                statement.execute(params![
                    observation.observation_id,
                    asset_path_string,
                    asset_id,
                    source_revision,
                    detector_fingerprint,
                    observation.local_index,
                    x,
                    y,
                    width,
                    height,
                    to_micro(observation.detection_score),
                    face.face_pixels,
                    to_micro(face.clarity.clamp(0.0, 1.0)),
                    landmarks_to_json(&observation.landmarks),
                ])?;
                embedding_statement.execute(params![
                    observation.observation_id,
                    embedder_fingerprint,
                    face.embedding.len() as i64,
                    encode_embedding(&face.embedding),
                ])?;
                inserted.push((observation.observation_id.clone(), bbox));
            }
        }

        // Checkpoint the scan even when no face was found. Without this an
        // asset with zero faces would be re-decoded on every run, and a killed
        // job could never resume.
        transaction.execute(
            "INSERT INTO face_asset_scans (
                asset_path, asset_id, source_revision, detector_fingerprint,
                face_count, analyzed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, unixepoch())
             ON CONFLICT(asset_path) DO UPDATE SET
                asset_id = excluded.asset_id,
                source_revision = excluded.source_revision,
                detector_fingerprint = excluded.detector_fingerprint,
                face_count = excluded.face_count,
                analyzed_at = excluded.analyzed_at",
            params![
                asset_path_string,
                asset_id,
                source_revision,
                detector_fingerprint,
                inserted.len() as i64,
            ],
        )?;

        // Re-bind user decisions to the overlapping new detection. A decision
        // whose face left the picture keeps its region and loses its
        // observation link instead of disappearing.
        for decision in decisions {
            let best = inserted
                .iter()
                .map(|(id, bbox)| (id, bbox.iou(decision.region)))
                .filter(|(_, iou)| *iou >= DECISION_REBIND_IOU)
                .max_by(|left, right| {
                    left.1
                        .partial_cmp(&right.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(id, _)| id.clone());
            if best != decision.observation_id {
                transaction.execute(
                    "UPDATE face_decisions SET observation_id = ?1, updated_at = unixepoch()
                     WHERE decision_id = ?2",
                    params![best, decision.decision_id],
                )?;
            }
        }

        for crop in crops {
            transaction.execute("INSERT OR REPLACE INTO face_crops(observation_id, size, policy_version, jpeg) VALUES (?1, ?2, ?3, ?4)", params![crop.observation_id, crop.size, FACE_CROP_POLICY_VERSION, crop.jpeg])?;
        }
        let revision = super::next_resource_revision(&transaction)?;
        let summary = serde_json::json!({
            "schemaVersion": 1, "analyzerId": "faces", "detectorFingerprint": detector_fingerprint,
            "embedderFingerprint": embedder_fingerprint, "faceCount": inserted.len(),
            "observationIds": inserted.iter().map(|(id, _)| id).collect::<Vec<_>>()
        })
        .to_string();
        transaction.execute(
            "INSERT INTO resource_projections(path, parent_path, projection_kind, source_revision, valid_at, state_revision, status, result_json)
             VALUES (?1, ?2, 'faces:v1', ?3, ?4, ?5, 'ready', ?6)
             ON CONFLICT(path, projection_kind) DO UPDATE SET source_revision=excluded.source_revision, valid_at=excluded.valid_at, state_revision=excluded.state_revision, status='ready', result_json=excluded.result_json, error=NULL",
            params![asset_path_string, asset_path.parent().unwrap_or(Path::new("")).to_string_lossy(), source_revision, valid_at.unwrap_or(revision), revision, summary],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Assets that still need analysis for `detector_fingerprint`.
    ///
    /// Resumable by construction: an asset is skipped once its scan checkpoint
    /// matches both the detector fingerprint and the source revision. A job
    /// killed mid-run therefore continues where it stopped, and an asset that
    /// genuinely contains no face is not decoded again on every run.
    pub fn face_analysis_targets(
        &self,
        detector_fingerprint: &str,
        force: bool,
        limit: usize,
    ) -> Result<Vec<FaceAnalysisTarget>, LibraryError> {
        self.face_analysis_targets_scoped("1 = 1", Vec::new(), detector_fingerprint, force, limit)
    }

    /// Targets inside one browsed directory.
    ///
    /// Matches the browser's own scope: opening a folder is non-recursive, so
    /// "analyze this folder" visits exactly the assets the grid is showing,
    /// however many pages of them exist.
    ///
    /// `root_path` and `directory` must be the paths the index stored — the
    /// session's canonical paths — or the comparison finds nothing.
    pub fn face_analysis_targets_in_directory(
        &self,
        root_path: &Path,
        directory: &Path,
        detector_fingerprint: &str,
        force: bool,
        limit: usize,
    ) -> Result<Vec<FaceAnalysisTarget>, LibraryError> {
        self.face_analysis_targets_scoped(
            "a.root_path = ?4 AND a.parent_path = ?5",
            vec![
                Box::new(root_path.to_string_lossy().to_string()),
                Box::new(directory.to_string_lossy().to_string()),
            ],
            detector_fingerprint,
            force,
            limit,
        )
    }

    /// Same checkpoint rules, restricted to explicit paths.
    pub fn face_analysis_targets_for_paths(
        &self,
        paths: &[PathBuf],
        detector_fingerprint: &str,
        force: bool,
    ) -> Result<Vec<FaceAnalysisTarget>, LibraryError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (4..paths.len() + 4)
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let scope = format!("a.path IN ({placeholders})");
        let values: Vec<Box<dyn rusqlite::ToSql>> = paths
            .iter()
            .map(|path| Box::new(path.to_string_lossy().to_string()) as Box<dyn rusqlite::ToSql>)
            .collect();
        self.face_analysis_targets_scoped(&scope, values, detector_fingerprint, force, usize::MAX)
    }

    /// One query for every scope, so the checkpoint rule exists once.
    ///
    /// `scope` is AND-ed with the checkpoint predicate and may reference
    /// parameters from `?4` onward.
    fn face_analysis_targets_scoped(
        &self,
        scope: &str,
        scope_values: Vec<Box<dyn rusqlite::ToSql>>,
        detector_fingerprint: &str,
        force: bool,
        limit: usize,
    ) -> Result<Vec<FaceAnalysisTarget>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        // The literal is interpolated from the shared contract so the scan
        // checkpoint can never disagree with what the analyzer records.
        let mut statement = connection.prepare(&format!(
            "SELECT a.path, a.kind, a.modified_at_ms, a.size_bytes
             FROM indexed_assets a
             LEFT JOIN face_asset_scans s ON s.asset_path = a.path
             WHERE (?1 = 1
                    OR s.asset_path IS NULL
                    OR s.detector_fingerprint != ?2
                    OR s.source_revision != (a.modified_at_ms || ':' || a.size_bytes || ':{FACE_REVISION_VERSION}'))
               AND {scope}
             ORDER BY a.path
             LIMIT ?3"
        ))?;
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(i64::from(force)),
            Box::new(detector_fingerprint.to_string()),
            Box::new(limit as i64),
        ];
        values.extend(scope_values);
        let borrowed: Vec<&dyn rusqlite::ToSql> = values.iter().map(AsRef::as_ref).collect();
        let rows = statement.query_map(borrowed.as_slice(), |row| {
            let modified_at_ms: i64 = row.get(2)?;
            let size_bytes: i64 = row.get(3)?;
            Ok((
                PathBuf::from(row.get::<_, String>(0)?),
                row.get::<_, String>(1)?,
                modified_at_ms,
                size_bytes,
            ))
        })?;
        let mut targets = Vec::new();
        for row in rows {
            let (path, kind, modified_at_ms, size_bytes) = row?;
            targets.push(FaceAnalysisTarget {
                path,
                kind: super::parse_kind(&kind)?,
                source_revision: face_source_revision(modified_at_ms as u64, size_bytes as u64),
                modified_at_ms: modified_at_ms as u64,
                size_bytes: size_bytes as u64,
            });
        }
        Ok(targets)
    }

    /// Confirmed embeddings grouped by person, normalized per gallery by the
    /// caller. This is the gallery a matcher searches.
    pub fn confirmed_face_embeddings(
        &self,
        embedder_fingerprint: &str,
    ) -> Result<Vec<(String, Vec<f32>)>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let mut statement = connection.prepare(
            "SELECT d.person_id, e.vector
             FROM face_decisions d
             JOIN face_embeddings e ON e.observation_id = d.observation_id
             WHERE d.decision_kind = 'confirm_person'
               AND d.person_id IS NOT NULL
               AND e.embedder_fingerprint = ?1
             ORDER BY d.person_id, e.observation_id",
        )?;
        let rows = statement.query_map(params![embedder_fingerprint], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut embeddings = Vec::new();
        for row in rows {
            let (person_id, bytes) = row?;
            embeddings.push((person_id, decode_embedding(&bytes)));
        }
        Ok(embeddings)
    }

    /// Embeddings of observations the user has not answered at all. Candidate
    /// matching may only propose a person for these: anything already decided
    /// is a user fact, including a rejection.
    pub fn unresolved_face_embeddings(
        &self,
        embedder_fingerprint: &str,
        limit: usize,
    ) -> Result<Vec<(FaceObservationId, Vec<f32>)>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let mut statement = connection.prepare(
            "SELECT e.observation_id, e.vector
             FROM face_embeddings e
             LEFT JOIN face_decisions d ON d.observation_id = e.observation_id
             WHERE e.embedder_fingerprint = ?1 AND d.decision_id IS NULL
             ORDER BY e.observation_id
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![embedder_fingerprint, limit as i64], |row| {
            Ok((
                row.get::<_, FaceObservationId>(0)?,
                row.get::<_, Vec<u8>>(1)?,
            ))
        })?;
        let mut embeddings = Vec::new();
        for row in rows {
            let (id, bytes) = row?;
            embeddings.push((id, decode_embedding(&bytes)));
        }
        Ok(embeddings)
    }

    /// `(analyzed, total)` indexed assets for one detector fingerprint, for job
    /// progress and resume reporting.
    pub fn face_analysis_counts(
        &self,
        detector_fingerprint: &str,
    ) -> Result<(u64, u64), LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let total: i64 =
            connection.query_row("SELECT COUNT(*) FROM indexed_assets", [], |row| row.get(0))?;
        let analyzed: i64 = connection.query_row(
            "SELECT COUNT(*) FROM face_asset_scans
             WHERE detector_fingerprint = ?1",
            params![detector_fingerprint],
            |row| row.get(0),
        )?;
        Ok((analyzed as u64, total as u64))
    }

    /// Replaces the person/decision projection with the durable set.
    ///
    /// This is the projection entry point when a `PersonStore` is present: the
    /// durable file is authoritative, and SQLite is derived from it. A full
    /// replace (rather than an incremental patch) means the projection cannot
    /// drift from the store, and it heals a partially applied mutation after a
    /// crash between the durable write and this call.
    ///
    /// `face_decision_events` is append-only history and is deliberately not
    /// truncated here.
    pub fn replace_user_data(
        &self,
        persons: &[PersonRecord],
        decisions: &[DecisionRecord],
    ) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM face_decisions", [])?;
        transaction.execute("DELETE FROM persons", [])?;
        {
            let mut person_statement = transaction.prepare_cached(
                "INSERT INTO persons (
                    person_id, display_name, name_key, linked_tag_id,
                    created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for person in persons {
                person_statement.execute(params![
                    person.person_id,
                    person.display_name,
                    person.display_name.to_lowercase(),
                    person.linked_tag_id,
                    person.created_at_ms as i64 / 1000,
                    person.updated_at_ms as i64 / 1000,
                ])?;
            }
            let mut decision_statement = transaction.prepare_cached(
                "INSERT INTO face_decisions (
                    observation_id, asset_path, asset_id, decision_kind, person_id,
                    x_micro, y_micro, width_micro, height_micro,
                    proposed_similarity_micro, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
            )?;
            let mut observations =
                std::collections::HashMap::<PathBuf, Vec<(String, NormalizedRect)>>::new();
            for record in decisions {
                let Some(region) = record.region.clamp_unit() else {
                    continue;
                };
                // Imported portable facts have no detector-local ID. Bind them
                // to existing observations without requiring another inference.
                let mut binding = record.observation_id.clone();
                {
                    if !observations.contains_key(&record.asset_path) {
                        let mut statement = transaction.prepare_cached("SELECT observation_id,x_micro,y_micro,width_micro,height_micro FROM face_observations WHERE asset_path=?1 ORDER BY observation_id")?;
                        let rows = statement
                            .query_map(params![record.asset_path.to_string_lossy()], |row| {
                                Ok((
                                    row.get(0)?,
                                    NormalizedRect {
                                        x: from_micro(row.get(1)?),
                                        y: from_micro(row.get(2)?),
                                        width: from_micro(row.get(3)?),
                                        height: from_micro(row.get(4)?),
                                    },
                                ))
                            })?
                            .collect::<Result<Vec<_>, _>>()?;
                        observations.insert(record.asset_path.clone(), rows);
                    }
                    let available = &observations[&record.asset_path];
                    if !available.is_empty()
                        && !available.iter().any(|(id, _)| Some(id) == binding.as_ref())
                    {
                        binding = available
                            .iter()
                            .map(|(id, bbox)| (id, bbox.iou(region)))
                            .filter(|(_, iou)| *iou >= DECISION_REBIND_IOU)
                            .max_by(|left, right| left.1.total_cmp(&right.1))
                            .map(|(id, _)| id.clone());
                    }
                }
                let (x, y, width, height) = rect_columns(region);
                decision_statement.execute(params![
                    binding,
                    record.asset_path.to_string_lossy(),
                    record.asset_id,
                    record.decision.kind(),
                    record.decision.person_id(),
                    x,
                    y,
                    width,
                    height,
                    record.proposed_similarity.map(to_micro),
                    record.created_at_ms as i64 / 1000,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Reads the current projection back out, so a store that does not exist yet
    /// can be seeded from user data recorded before the store existed.
    pub fn export_user_data(
        &self,
    ) -> Result<(Vec<PersonRecord>, Vec<DecisionRecord>), LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let persons = {
            let mut statement = connection.prepare(
                "SELECT person_id, display_name, linked_tag_id, created_at, updated_at
                 FROM persons ORDER BY person_id",
            )?;
            statement
                .query_map([], |row| {
                    Ok(PersonRecord {
                        tag_path: None,
                        person_id: row.get(0)?,
                        display_name: row.get(1)?,
                        linked_tag_id: row.get(2)?,
                        created_at_ms: row.get::<_, i64>(3)? as u64 * 1000,
                        updated_at_ms: row.get::<_, i64>(4)? as u64 * 1000,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        let decisions = {
            let mut statement = connection.prepare(
                "SELECT observation_id, asset_id, asset_path, decision_kind, person_id,
                        x_micro, y_micro, width_micro, height_micro, created_at,
                        proposed_similarity_micro
                 FROM face_decisions ORDER BY decision_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    rect_from_row(row, 5)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                ))
            })?;
            let mut decisions = Vec::new();
            for row in rows {
                let (
                    observation_id,
                    asset_id,
                    asset_path,
                    kind,
                    person_id,
                    region,
                    created,
                    similarity,
                ) = row?;
                let Some(decision) = parse_decision(&kind, person_id) else {
                    continue;
                };
                decisions.push(DecisionRecord {
                    observation_id,
                    asset_id,
                    asset_path: PathBuf::from(asset_path),
                    region,
                    decision,
                    created_at_ms: created as u64 * 1000,
                    proposed_similarity: similarity.map(from_micro),
                });
            }
            decisions
        };
        Ok((persons, decisions))
    }

    /// Moves the machine-side face cache to a renamed or moved asset.
    ///
    /// Only observations and the scan checkpoint move: user decisions live in
    /// the durable store, which the Host transfers and then projects. The
    /// observation ids stay as they are — a cache id that no longer matches what
    /// a fresh analysis would derive is harmless, and the next analysis replaces
    /// both the rows and the decision bindings by region.
    pub fn move_asset_face_state(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), LibraryError> {
        let asset_id = oxy_fs::stable_asset_id(destination);
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let source_string = source.to_string_lossy().to_string();
        let destination_string = destination.to_string_lossy().to_string();
        transaction.execute(
            "UPDATE face_observations SET asset_path = ?1, asset_id = ?2 WHERE asset_path = ?3",
            params![destination_string, asset_id, source_string],
        )?;
        transaction.execute(
            "UPDATE face_asset_scans SET asset_path = ?1, asset_id = ?2 WHERE asset_path = ?3",
            params![destination_string, asset_id, source_string],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Drops the machine-side face cache for an asset that no longer exists.
    ///
    /// User decisions are handled by the durable store, so this never touches
    /// `face_decisions`.
    pub fn remove_asset_face_state(&self, path: &Path) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let path_string = path.to_string_lossy().to_string();
        transaction.execute(
            "DELETE FROM face_observations WHERE asset_path = ?1",
            params![path_string],
        )?;
        transaction.execute(
            "DELETE FROM face_asset_scans WHERE asset_path = ?1",
            params![path_string],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// The detector fingerprint an asset was last analyzed with, so a caller can
    /// skip work that is already current.
    pub fn asset_face_fingerprint(
        &self,
        asset_path: &Path,
    ) -> Result<Option<String>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let value = connection
            .query_row(
                "SELECT detector_fingerprint FROM face_observations
                 WHERE asset_path = ?1 LIMIT 1",
                params![asset_path.to_string_lossy()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(value)
    }

    pub fn face_observations_for_asset(
        &self,
        asset_path: &Path,
    ) -> Result<Vec<FaceObservation>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let mut statement = connection.prepare(
            "SELECT observation_id, asset_id, source_revision, detector_fingerprint,
                    local_index, x_micro, y_micro, width_micro, height_micro,
                    detection_score_micro, landmarks_json
             FROM face_observations WHERE asset_path = ?1
             ORDER BY local_index",
        )?;
        let asset_path_string = asset_path.to_string_lossy().to_string();
        let observations = statement
            .query_map(params![asset_path_string], |row| {
                Ok(FaceObservation {
                    observation_id: row.get(0)?,
                    asset_id: row.get(1)?,
                    asset_path: asset_path.to_path_buf(),
                    source_revision: row.get(2)?,
                    detector_fingerprint: row.get(3)?,
                    local_index: row.get(4)?,
                    bbox: rect_from_row(row, 5)?,
                    detection_score: from_micro(row.get(9)?),
                    landmarks: landmarks_from_json(&row.get::<_, String>(10)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(observations)
    }

    /// The pending proposal for one observation, if the matcher made one.
    ///
    /// The caller records its score with the user's answer, which is how the
    /// application learns how the current threshold behaves on this library.
    pub fn pending_candidate_for(
        &self,
        observation_id: &str,
    ) -> Result<Option<FaceCandidate>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        Ok(pending_candidate(&connection, observation_id)?)
    }

    /// One observation by id, used when a user decision must be recorded
    /// against the region the face actually occupies.
    pub fn face_observation(
        &self,
        observation_id: &str,
    ) -> Result<Option<FaceObservation>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let stored = connection
            .query_row(
                "SELECT observation_id, asset_id, asset_path, source_revision,
                        detector_fingerprint, local_index, x_micro, y_micro,
                        width_micro, height_micro, detection_score_micro, landmarks_json
                 FROM face_observations WHERE observation_id = ?1",
                params![observation_id],
                |row| {
                    Ok(FaceObservation {
                        observation_id: row.get(0)?,
                        asset_id: row.get(1)?,
                        asset_path: PathBuf::from(row.get::<_, String>(2)?),
                        source_revision: row.get(3)?,
                        detector_fingerprint: row.get(4)?,
                        local_index: row.get(5)?,
                        bbox: rect_from_row(row, 6)?,
                        detection_score: from_micro(row.get(10)?),
                        landmarks: landmarks_from_json(&row.get::<_, String>(11)?),
                    })
                },
            )
            .optional()?;
        Ok(stored)
    }

    /// Every observation the user has not identified yet, with its embedding.
    /// This is the input to clustering: known people are excluded so a new run
    /// cannot reshuffle them, and a `not_face` region is excluded so a
    /// suppressed face is not offered again.
    pub fn undecided_face_embeddings(
        &self,
        embedder_fingerprint: &str,
        limit: usize,
    ) -> Result<Vec<(FaceObservationId, Vec<f32>)>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let mut statement = connection.prepare(
            "SELECT e.observation_id, e.vector
             FROM face_embeddings e
             LEFT JOIN face_decisions d ON d.observation_id = e.observation_id
             WHERE e.embedder_fingerprint = ?1
               AND (d.decision_id IS NULL
                    OR d.decision_kind NOT IN ('confirm_person', 'not_face'))
             ORDER BY e.observation_id
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![embedder_fingerprint, limit as i64], |row| {
            Ok((
                row.get::<_, FaceObservationId>(0)?,
                row.get::<_, Vec<u8>>(1)?,
            ))
        })?;
        let mut embeddings = Vec::new();
        for row in rows {
            let (id, bytes) = row?;
            embeddings.push((id, decode_embedding(&bytes)));
        }
        Ok(embeddings)
    }

    /// Unknown-face clustering input with the source asset identity required
    /// for the hard rule that two faces in one photo cannot be one person.
    pub fn undecided_face_cluster_embeddings(
        &self,
        embedder_fingerprint: &str,
        limit: usize,
    ) -> Result<Vec<(FaceObservationId, String, Vec<f32>)>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let mut statement = connection.prepare(
            "SELECT e.observation_id, o.asset_id, e.vector
             FROM face_embeddings e
             JOIN face_observations o ON o.observation_id = e.observation_id
             LEFT JOIN face_decisions d ON d.observation_id = e.observation_id
             WHERE e.embedder_fingerprint = ?1
               AND (d.decision_id IS NULL
                    OR d.decision_kind NOT IN ('confirm_person', 'not_face'))
             ORDER BY e.observation_id
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![embedder_fingerprint, limit as i64], |row| {
            Ok((
                row.get::<_, FaceObservationId>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut embeddings = Vec::new();
        for row in rows {
            let (observation_id, asset_id, bytes) = row?;
            embeddings.push((observation_id, asset_id, decode_embedding(&bytes)));
        }
        Ok(embeddings)
    }

    /// Publish both derived views together, only if their settings are still current.
    pub fn replace_face_derived(
        &self,
        settings: &FaceAnalyzerSettings,
        clustering_fingerprint: &str,
        clusters: &[FaceCluster],
        matcher_fingerprint: &str,
        candidates: &[FaceCandidate],
        cancellation: &oxy_runtime::CancellationToken,
    ) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        if cancellation.is_cancelled() {
            return Err(LibraryError::StaleFaceResult);
        }
        transaction.execute("DELETE FROM face_cluster_members", [])?;
        transaction.execute("DELETE FROM face_clusters", [])?;
        transaction.execute("DELETE FROM face_candidates", [])?;
        write_clusters(&transaction, clustering_fingerprint, clusters)?;
        write_candidates(&transaction, matcher_fingerprint, candidates)?;
        // Settings changes cancel the same token before publication. Recheck
        // after writes so cancellation rolls the whole transaction back.
        let json: Option<String> = transaction
            .query_row(
                "SELECT settings_json FROM face_settings WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let current = json
            .map(|json| serde_json::from_str::<FaceAnalyzerSettings>(&json))
            .transpose()?
            .unwrap_or_default()
            .sanitized();
        if current != *settings {
            return Err(LibraryError::StaleFaceResult);
        }
        if cancellation.is_cancelled() {
            return Err(LibraryError::StaleFaceResult);
        }
        transaction.commit()?;
        Ok(())
    }

    /// Replaces the cluster cache for one clustering fingerprint. Clusters are
    /// rebuildable, so a full replacement is safe; decisions are untouched.
    pub fn replace_face_clusters(
        &self,
        clustering_fingerprint: &str,
        clusters: &[FaceCluster],
    ) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        write_clusters(&transaction, clustering_fingerprint, clusters)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn face_clusters(
        &self,
        clustering_fingerprint: &str,
    ) -> Result<Vec<FaceCluster>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let mut statement = connection.prepare(
            "SELECT cluster_id, representative_observation_id, member_count,
                    cohesion_micro, outlier_ids_json
             FROM face_clusters WHERE clustering_fingerprint = ?1
             ORDER BY member_count DESC, cluster_id",
        )?;
        let mut clusters: Vec<FaceCluster> = statement
            .query_map(params![clustering_fingerprint], |row| {
                let outlier_json: String = row.get(4)?;
                Ok(FaceCluster {
                    cluster_id: row.get(0)?,
                    representative_observation_id: row.get(1)?,
                    member_count: row.get::<_, i64>(2)? as usize,
                    cohesion: from_micro(row.get(3)?),
                    outlier_observation_ids: serde_json::from_str(&outlier_json)
                        .unwrap_or_default(),
                    observation_ids: Vec::new(),
                    suggested_person_id: None,
                    suggested_name: None,
                    member_quality: Vec::new(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut member_statement = connection.prepare(
            "SELECT m.observation_id, o.detection_score_micro,
                    o.face_pixels, o.clarity_score_micro
             FROM face_cluster_members m
             JOIN face_observations o ON o.observation_id = m.observation_id
             WHERE m.cluster_id = ?1 AND m.clustering_fingerprint = ?2
             ORDER BY m.observation_id",
        )?;
        for cluster in &mut clusters {
            cluster.member_quality = member_statement
                .query_map(params![cluster.cluster_id, clustering_fingerprint], |row| {
                    Ok(oxy_domain::FaceQuality {
                        observation_id: row.get(0)?,
                        detection_score: from_micro(row.get(1)?),
                        face_pixels: row.get::<_, i64>(2)? as u32,
                        clarity: from_micro(row.get(3)?),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            cluster.observation_ids = cluster
                .member_quality
                .iter()
                .map(|member| member.observation_id.clone())
                .collect();
        }
        Ok(clusters)
    }

    /// Clusters from the most recent clustering pass.
    ///
    /// The caller does not need to know the clustering fingerprint, which
    /// encodes the embedder and thresholds; only one pass is current at a time.
    pub fn face_clusters_current(&self) -> Result<Vec<FaceCluster>, LibraryError> {
        let fingerprint: Option<String> = {
            let mut reader = self.read_connection();
            let connection = reader.transaction()?;
            connection
                .query_row(
                    "SELECT clustering_fingerprint FROM face_clusters
                     GROUP BY clustering_fingerprint
                     ORDER BY MAX(created_at) DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
        };
        match fingerprint {
            Some(fingerprint) => self.face_clusters(&fingerprint),
            None => Ok(Vec::new()),
        }
    }

    /// Replaces matcher proposals for one matcher fingerprint. A rejected
    /// proposal is represented by the durable rejection in `face_decisions`, so
    /// dropping the cached row here cannot resurrect it.
    pub fn replace_face_candidates(
        &self,
        matcher_fingerprint: &str,
        candidates: &[FaceCandidate],
    ) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        write_candidates(&transaction, matcher_fingerprint, candidates)?;
        transaction.commit()?;
        Ok(())
    }

    /// Creates a person. The id is supplied by the caller so an undo journal or
    /// an import can reproduce it.
    pub fn create_person(
        &self,
        person_id: &str,
        display_name: &str,
        linked_tag_id: Option<CustomTagId>,
    ) -> Result<Person, LibraryError> {
        let name = display_name.trim();
        if name.is_empty() {
            return Err(LibraryError::InvalidPersonName);
        }
        let connection = self.connection.lock();
        connection.execute(
            "INSERT INTO persons (person_id, display_name, name_key, linked_tag_id)
             VALUES (?1, ?2, ?3, ?4)",
            params![person_id, name, name.to_lowercase(), linked_tag_id],
        )?;
        Ok(Person {
            person_id: person_id.to_string(),
            display_name: name.to_string(),
            linked_tag_id,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            face_count: 0,
            cover_observation_id: None,
        })
    }

    /// Renames a person. Renaming never touches observations, embeddings, or
    /// decisions: the person id is the identity, the name is a label.
    pub fn rename_person(&self, person_id: &str, display_name: &str) -> Result<(), LibraryError> {
        let name = display_name.trim();
        if name.is_empty() {
            return Err(LibraryError::InvalidPersonName);
        }
        let connection = self.connection.lock();
        let updated = connection.execute(
            "UPDATE persons SET display_name = ?1, name_key = ?2, updated_at = unixepoch()
             WHERE person_id = ?3",
            params![name, name.to_lowercase(), person_id],
        )?;
        if updated == 0 {
            return Err(LibraryError::MissingPerson(person_id.to_string()));
        }
        Ok(())
    }

    pub fn link_person_tag(
        &self,
        person_id: &str,
        tag_id: Option<CustomTagId>,
    ) -> Result<(), LibraryError> {
        let connection = self.connection.lock();
        let updated = connection.execute(
            "UPDATE persons SET linked_tag_id = ?1, updated_at = unixepoch() WHERE person_id = ?2",
            params![tag_id, person_id],
        )?;
        if updated == 0 {
            return Err(LibraryError::MissingPerson(person_id.to_string()));
        }
        Ok(())
    }

    /// Deletes a person and the confirmations that named it.
    ///
    /// `NotFace` decisions survive: they are about a region, not about a
    /// person. Rejections of the deleted person are removed because a rejection
    /// without its subject would block nothing and only confuse the queue.
    pub fn delete_person(&self, person_id: &str) -> Result<usize, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let removed = transaction.execute(
            "DELETE FROM face_decisions WHERE person_id = ?1",
            params![person_id],
        )?;
        let deleted = transaction.execute(
            "DELETE FROM persons WHERE person_id = ?1",
            params![person_id],
        )?;
        transaction.commit()?;
        if deleted == 0 {
            return Err(LibraryError::MissingPerson(person_id.to_string()));
        }
        Ok(removed)
    }

    /// People with their confirmation count.
    ///
    /// `face_count` counts durable confirmations for the person, not
    /// observations that are currently bound. After a cache rebuild the count
    /// is therefore already correct while `cover_observation_id` is still
    /// empty; the count is user data and the binding is cache.
    pub fn persons(&self) -> Result<Vec<Person>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let mut statement = connection.prepare(
            "SELECT p.person_id, p.display_name, p.linked_tag_id, p.created_at, p.updated_at,
                    (SELECT COUNT(*) FROM face_decisions d
                     WHERE d.person_id = p.person_id AND d.decision_kind = 'confirm_person'),
                    (SELECT d2.observation_id FROM face_decisions d2
                     JOIN face_observations o2 ON o2.observation_id = d2.observation_id
                     WHERE d2.person_id = p.person_id AND d2.decision_kind = 'confirm_person'
                     ORDER BY d2.updated_at DESC LIMIT 1)
             FROM persons p
             ORDER BY p.display_name COLLATE NOCASE",
        )?;
        let persons = statement
            .query_map([], |row| {
                Ok(Person {
                    person_id: row.get(0)?,
                    display_name: row.get(1)?,
                    linked_tag_id: row.get(2)?,
                    created_at_ms: row.get::<_, i64>(3)? as u64 * 1000,
                    updated_at_ms: row.get::<_, i64>(4)? as u64 * 1000,
                    face_count: row.get::<_, i64>(5)? as usize,
                    cover_observation_id: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(persons)
    }

    /// Records a user decision about one detected face.
    ///
    /// The observation must exist: a decision is always about a face the user
    /// could see. Afterwards it is durable even if a later analysis run
    /// replaces the observation.
    pub fn record_face_decision(
        &self,
        observation_id: &str,
        decision: &FaceDecision,
        proposed_similarity: Option<f32>,
    ) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let existing: Option<(String, String, NormalizedRect)> = transaction
            .query_row(
                "SELECT asset_path, asset_id, x_micro, y_micro, width_micro, height_micro
                 FROM face_observations WHERE observation_id = ?1",
                params![observation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        rect_from_row(row, 2)?,
                    ))
                },
            )
            .optional()?;
        let Some((asset_path, asset_id, region)) = existing else {
            return Err(LibraryError::MissingFaceObservation(
                observation_id.to_string(),
            ));
        };
        let person_id = decision.person_id().map(str::to_string);
        if let Some(person_id) = person_id.as_deref() {
            let known: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM persons WHERE person_id = ?1)",
                params![person_id],
                |row| row.get(0),
            )?;
            if !known {
                return Err(LibraryError::MissingPerson(person_id.to_string()));
            }
        }
        let (x, y, width, height) = rect_columns(region);

        // One decision per observation: re-answering replaces the previous one
        // while the event log keeps the history for undo.
        transaction.execute(
            "DELETE FROM face_decisions WHERE observation_id = ?1",
            params![observation_id],
        )?;
        transaction.execute(
            "INSERT INTO face_decisions (
                observation_id, asset_path, asset_id, decision_kind, person_id,
                x_micro, y_micro, width_micro, height_micro, proposed_similarity_micro
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                observation_id,
                asset_path,
                asset_id,
                decision.kind(),
                person_id,
                x,
                y,
                width,
                height,
                proposed_similarity.map(to_micro),
            ],
        )?;
        let decision_id = transaction.last_insert_rowid();
        transaction.execute(
            "INSERT INTO face_decision_events (
                decision_id, observation_id, decision_kind, person_id,
                x_micro, y_micro, width_micro, height_micro, payload_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                decision_id,
                observation_id,
                decision.kind(),
                person_id,
                x,
                y,
                width,
                height,
                serde_json::to_string(decision)?,
            ],
        )?;
        // A resolved face is no longer a pending proposal.
        transaction.execute(
            "DELETE FROM face_candidates WHERE observation_id = ?1",
            params![observation_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Removes the user's answer for one face, returning it to the queue.
    pub fn clear_face_decision(&self, observation_id: &str) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let removed = transaction.execute(
            "DELETE FROM face_decisions WHERE observation_id = ?1",
            params![observation_id],
        )?;
        if removed > 0 {
            transaction.execute(
                "INSERT INTO face_decision_events (
                    decision_id, observation_id, decision_kind, payload_json
                 ) VALUES (NULL, ?1, 'cleared', '{}')",
                params![observation_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Every durable decision, including ones whose observation was replaced.
    /// This is what a durable-store sync writes to disk.
    pub fn face_decisions(&self) -> Result<Vec<StoredDecision>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let mut statement = connection.prepare(
            "SELECT decision_id, observation_id, asset_id, asset_path,
                    decision_kind, person_id, x_micro, y_micro, width_micro, height_micro
             FROM face_decisions ORDER BY decision_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                rect_from_row(row, 6)?,
            ))
        })?;
        let mut decisions = Vec::new();
        for row in rows {
            let (decision_id, observation_id, asset_id, asset_path, kind, person_id, region) = row?;
            let Some(decision) = parse_decision(&kind, person_id) else {
                continue;
            };
            decisions.push(StoredDecision {
                decision_id,
                observation_id,
                asset_id,
                asset_path: PathBuf::from(asset_path),
                region,
                decision,
            });
        }
        Ok(decisions)
    }

    /// Append-only user actions for undo and audit, newest first.
    pub fn face_decision_records(
        &self,
        limit: usize,
    ) -> Result<Vec<FaceDecisionRecord>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let mut statement = connection.prepare(
            "SELECT event_id, observation_id, decision_kind, person_id,
                    x_micro, y_micro, width_micro, height_micro, created_at
             FROM face_decision_events
             WHERE observation_id IS NOT NULL AND decision_kind != 'cleared'
             ORDER BY event_id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                rect_from_row(row, 4)?,
                row.get::<_, i64>(8)?,
            ))
        })?;
        let mut records = Vec::new();
        for row in rows {
            let (event_id, observation_id, kind, person_id, region, created_at) = row?;
            let Some(decision) = parse_decision(&kind, person_id) else {
                continue;
            };
            records.push(FaceDecisionRecord {
                event_id,
                observation_id,
                decision,
                region,
                created_at_ms: created_at as u64 * 1000,
            });
        }
        Ok(records)
    }

    /// Builds one page of the review queue.
    ///
    /// Precedence is user fact over machine proposal: a stored decision decides
    /// the state, and a candidate is only reported when nothing was decided.
    pub fn face_review_page(
        &self,
        filter: FaceReviewFilter,
        offset: usize,
        limit: usize,
    ) -> Result<FaceReviewPage, LibraryError> {
        self.face_review_page_filtered(filter, offset, limit, 0.0, 0, 0.0)
    }

    pub fn face_review_page_filtered(
        &self,
        filter: FaceReviewFilter,
        offset: usize,
        limit: usize,
        min_detection_score: f32,
        min_face_pixels: u32,
        min_clarity: f32,
    ) -> Result<FaceReviewPage, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let min_detection_score = if min_detection_score.is_finite() {
            min_detection_score.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let min_clarity = if min_clarity.is_finite() {
            min_clarity.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let predicate = format!(
            "({}) AND o.detection_score_micro >= {} AND o.face_pixels >= {} AND o.clarity_score_micro >= {}",
            filter_predicate(filter),
            to_micro(min_detection_score),
            min_face_pixels,
            to_micro(min_clarity),
        );
        let total: i64 = connection.query_row(
            &format!(
                "SELECT COUNT(*) FROM face_observations o
                 LEFT JOIN face_decisions d ON d.observation_id = o.observation_id
                 WHERE {predicate}"
            ),
            [],
            |row| row.get(0),
        )?;
        let mut statement = connection.prepare(&format!(
            "SELECT o.observation_id, o.asset_id, o.asset_path,
                    o.x_micro, o.y_micro, o.width_micro, o.height_micro,
                    o.detection_score_micro, o.face_pixels, o.clarity_score_micro,
                    d.decision_kind, d.person_id, p.display_name,
                    (SELECT c.cluster_id FROM face_cluster_members c
                     WHERE c.observation_id = o.observation_id LIMIT 1)
             FROM face_observations o
             LEFT JOIN face_decisions d ON d.observation_id = o.observation_id
             LEFT JOIN persons p ON p.person_id = d.person_id
             WHERE {predicate}
             ORDER BY o.asset_path COLLATE NOCASE, o.local_index
             LIMIT ?1 OFFSET ?2"
        ))?;
        let rows = statement
            .query_map(params![limit as i64, offset as i64], |row| {
                Ok((
                    row.get::<_, FaceObservationId>(0)?,
                    row.get::<_, AssetId>(1)?,
                    row.get::<_, String>(2)?,
                    rect_from_row(row, 3)?,
                    from_micro(row.get::<_, i64>(7)?),
                    row.get::<_, i64>(8)? as u32,
                    from_micro(row.get::<_, i64>(9)?),
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut items = Vec::with_capacity(rows.len());
        for (
            observation_id,
            asset_id,
            asset_path,
            bbox,
            detection_score,
            face_pixels,
            clarity,
            kind,
            person_id,
            person_name,
            cluster_id,
        ) in rows
        {
            let mut state = FaceReviewState::Unknown;
            let mut confirmed_person_id = None;
            let mut confirmed_person_name = None;
            if let Some(kind) = kind.as_deref() {
                match kind {
                    "confirm_person" => {
                        state = FaceReviewState::Confirmed;
                        confirmed_person_id = person_id.clone();
                        confirmed_person_name = person_name.clone();
                    }
                    "reject_person" => state = FaceReviewState::Rejected,
                    "not_face" => state = FaceReviewState::NotFace,
                    _ => {}
                }
            }
            let candidate = if state == FaceReviewState::Unknown {
                pending_candidate(&connection, &observation_id)?
            } else {
                None
            };
            if candidate.is_some() {
                state = FaceReviewState::Pending;
            }
            items.push(FaceReviewItem {
                observation_id,
                asset_id,
                asset_path: PathBuf::from(asset_path),
                bbox,
                detection_score,
                face_pixels,
                clarity,
                manual_blurry: None,
                state,
                candidate,
                cluster_id,
                confirmed_person_id,
                confirmed_person_name,
            });
        }
        let consumed = offset + items.len();
        let next_cursor = (consumed < total as usize).then_some(consumed);
        Ok(FaceReviewPage {
            items,
            total: total as usize,
            next_cursor,
        })
    }

    /// Review rows for one asset, in the same shape as [`Library::face_review_page`].
    ///
    /// The loupe uses this to draw face boxes with the user's current answer, so
    /// it never has to join observations and decisions itself.
    pub fn face_reviews_for_asset(
        &self,
        asset_path: &Path,
    ) -> Result<Vec<FaceReviewItem>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let mut statement = connection.prepare(
            "SELECT o.observation_id, o.asset_id, o.asset_path,
                    o.x_micro, o.y_micro, o.width_micro, o.height_micro,
                    o.detection_score_micro, o.face_pixels, o.clarity_score_micro,
                    d.decision_kind, d.person_id, p.display_name,
                    (SELECT c.cluster_id FROM face_cluster_members c
                     WHERE c.observation_id = o.observation_id LIMIT 1)
             FROM face_observations o
             LEFT JOIN face_decisions d ON d.observation_id = o.observation_id
             LEFT JOIN persons p ON p.person_id = d.person_id
             WHERE o.asset_path = ?1
             ORDER BY o.local_index",
        )?;
        let rows = statement
            .query_map(params![asset_path.to_string_lossy()], |row| {
                Ok((
                    row.get::<_, FaceObservationId>(0)?,
                    row.get::<_, AssetId>(1)?,
                    row.get::<_, String>(2)?,
                    rect_from_row(row, 3)?,
                    from_micro(row.get::<_, i64>(7)?),
                    row.get::<_, i64>(8)? as u32,
                    from_micro(row.get::<_, i64>(9)?),
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut items = Vec::with_capacity(rows.len());
        for (
            observation_id,
            asset_id,
            asset_path,
            bbox,
            detection_score,
            face_pixels,
            clarity,
            kind,
            person_id,
            person_name,
            cluster_id,
        ) in rows
        {
            let mut state = FaceReviewState::Unknown;
            let mut confirmed_person_id = None;
            let mut confirmed_person_name = None;
            if let Some(kind) = kind.as_deref() {
                match kind {
                    "confirm_person" => {
                        state = FaceReviewState::Confirmed;
                        confirmed_person_id = person_id.clone();
                        confirmed_person_name = person_name.clone();
                    }
                    "reject_person" => state = FaceReviewState::Rejected,
                    "not_face" => state = FaceReviewState::NotFace,
                    _ => {}
                }
            }
            let candidate = if state == FaceReviewState::Unknown {
                pending_candidate(&connection, &observation_id)?
            } else {
                None
            };
            if candidate.is_some() {
                state = FaceReviewState::Pending;
            }
            items.push(FaceReviewItem {
                observation_id,
                asset_id,
                asset_path: PathBuf::from(asset_path),
                bbox,
                detection_score,
                face_pixels,
                clarity,
                manual_blurry: None,
                state,
                candidate,
                cluster_id,
                confirmed_person_id,
                confirmed_person_name,
            });
        }
        Ok(items)
    }

    pub fn face_library_stats(&self) -> Result<FaceLibraryStats, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let count = |sql: &str| -> Result<i64, LibraryError> {
            Ok(connection.query_row(sql, [], |row| row.get(0))?)
        };
        Ok(FaceLibraryStats {
            analyzed_assets: count("SELECT COUNT(DISTINCT asset_path) FROM face_observations")?
                as u64,
            faces_detected: count("SELECT COUNT(*) FROM face_observations")? as u64,
            persons: count("SELECT COUNT(*) FROM persons")? as u64,
            pending_reviews: count(
                "SELECT COUNT(*) FROM face_observations o
                 JOIN face_candidates c ON c.observation_id = o.observation_id
                 LEFT JOIN face_decisions d ON d.observation_id = o.observation_id
                 WHERE d.decision_id IS NULL AND c.state = 'pending'",
            )? as u64,
            unknown_faces: count(
                "SELECT COUNT(*) FROM face_observations o
                 LEFT JOIN face_decisions d ON d.observation_id = o.observation_id
                 WHERE d.decision_id IS NULL
                   AND NOT EXISTS (SELECT 1 FROM face_candidates c
                                   WHERE c.observation_id = o.observation_id
                                     AND c.state = 'pending')",
            )? as u64,
            clusters: count("SELECT COUNT(DISTINCT cluster_id) FROM face_clusters")? as u64,
        })
    }

    pub fn face_analyzer_settings(&self) -> Result<FaceAnalyzerSettings, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let stored: Option<String> = connection
            .query_row(
                "SELECT settings_json FROM face_settings WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(stored
            .and_then(|json| serde_json::from_str::<FaceAnalyzerSettings>(&json).ok())
            .unwrap_or_default()
            .sanitized())
    }

    pub fn update_face_analyzer_settings(
        &self,
        settings: &FaceAnalyzerSettings,
    ) -> Result<FaceAnalyzerSettings, LibraryError> {
        let settings = (*settings).sanitized();
        let connection = self.connection.lock();
        connection.execute(
            "INSERT INTO face_settings (id, settings_json, updated_at)
             VALUES (1, ?1, unixepoch())
             ON CONFLICT(id) DO UPDATE SET
                settings_json = excluded.settings_json,
                updated_at = excluded.updated_at",
            params![serde_json::to_string(&settings)?],
        )?;
        Ok(settings)
    }

    /// Withdraws a cached proposal without creating a durable decision.
    pub fn mark_candidate_rejected(
        &self,
        observation_id: &str,
        person_id: &str,
    ) -> Result<(), LibraryError> {
        let connection = self.connection.lock();
        connection.execute(
            "UPDATE face_candidates SET state = 'rejected'
             WHERE observation_id = ?1 AND person_id = ?2",
            params![observation_id, person_id],
        )?;
        Ok(())
    }

    /// Display-oriented dimensions for an asset, read from the cached image
    /// projection. `None` only means the dimensions are not known yet.
    pub fn face_asset_pixel_size(
        &self,
        asset_path: &Path,
    ) -> Result<Option<PixelSize>, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let stored: Option<String> = connection
            .query_row(
                "SELECT result_json FROM resource_projections
                 WHERE path = ?1 AND projection_kind = 'image:preview'",
                params![asset_path.to_string_lossy()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(json) = stored else {
            return Ok(None);
        };
        let value: serde_json::Value = serde_json::from_str(&json)?;
        let width = value.get("width").and_then(serde_json::Value::as_u64);
        let height = value.get("height").and_then(serde_json::Value::as_u64);
        Ok(match (width, height) {
            (Some(width), Some(height)) if width > 0 && height > 0 => Some(PixelSize {
                width: width as u32,
                height: height as u32,
            }),
            _ => None,
        })
    }

    /// Number of stored observations for an asset, regardless of fingerprint.
    pub fn face_observation_count(&self, asset_path: &Path) -> Result<usize, LibraryError> {
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM face_observations WHERE asset_path = ?1",
            params![asset_path.to_string_lossy()],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}

/// A stored decision as read for re-binding during re-analysis.
struct RebindableDecision {
    decision_id: i64,
    observation_id: Option<String>,
    region: NormalizedRect,
}

fn decisions_for_asset(
    transaction: &Transaction<'_>,
    asset_path: &str,
) -> Result<Vec<RebindableDecision>, LibraryError> {
    let mut statement = transaction.prepare(
        "SELECT decision_id, observation_id, decision_kind, person_id,
                x_micro, y_micro, width_micro, height_micro
         FROM face_decisions WHERE asset_path = ?1",
    )?;
    let rows = statement.query_map(params![asset_path], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            rect_from_row(row, 4)?,
        ))
    })?;
    let mut decisions = Vec::new();
    for row in rows {
        let (decision_id, observation_id, kind, person_id, region) = row?;
        if parse_decision(&kind, person_id).is_some() {
            decisions.push(RebindableDecision {
                decision_id,
                observation_id,
                region,
            });
        }
    }
    Ok(decisions)
}

fn pending_candidate(
    connection: &Connection,
    observation_id: &str,
) -> Result<Option<FaceCandidate>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT c.observation_id, c.person_id, p.display_name,
                    c.similarity_micro, c.matcher_fingerprint
             FROM face_candidates c
             JOIN persons p ON p.person_id = c.person_id
             WHERE c.observation_id = ?1 AND c.state = 'pending'
             ORDER BY c.similarity_micro DESC LIMIT 1",
            params![observation_id],
            |row| {
                Ok(FaceCandidate {
                    observation_id: row.get(0)?,
                    person_id: row.get(1)?,
                    person_name: row.get(2)?,
                    similarity: from_micro(row.get(3)?),
                    matcher_fingerprint: row.get(4)?,
                })
            },
        )
        .optional()
}

fn write_clusters(
    transaction: &Transaction<'_>,
    clustering_fingerprint: &str,
    clusters: &[FaceCluster],
) -> Result<(), LibraryError> {
    transaction.execute(
        "DELETE FROM face_cluster_members WHERE clustering_fingerprint = ?1",
        params![clustering_fingerprint],
    )?;
    transaction.execute(
        "DELETE FROM face_clusters WHERE clustering_fingerprint = ?1",
        params![clustering_fingerprint],
    )?;
    {
        let mut cluster_statement = transaction.prepare_cached(
            "INSERT INTO face_clusters (
                    cluster_id, clustering_fingerprint, representative_observation_id,
                    member_count, cohesion_micro, outlier_ids_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut member_statement = transaction.prepare_cached(
            "INSERT OR IGNORE INTO face_cluster_members (
                    cluster_id, clustering_fingerprint, observation_id
                 ) VALUES (?1, ?2, ?3)",
        )?;
        for cluster in clusters {
            cluster_statement.execute(params![
                cluster.cluster_id,
                clustering_fingerprint,
                cluster.representative_observation_id,
                cluster.member_count as i64,
                to_micro(cluster.cohesion),
                serde_json::to_string(&cluster.outlier_observation_ids)?,
            ])?;
            for member in &cluster.observation_ids {
                member_statement.execute(params![
                    cluster.cluster_id,
                    clustering_fingerprint,
                    member
                ])?;
            }
        }
    }
    Ok(())
}

fn write_candidates(
    transaction: &Transaction<'_>,
    matcher_fingerprint: &str,
    candidates: &[FaceCandidate],
) -> Result<(), LibraryError> {
    transaction.execute(
        "DELETE FROM face_candidates WHERE matcher_fingerprint = ?1",
        params![matcher_fingerprint],
    )?;
    {
        // A user decision outranks a machine proposal structurally: the
        // insert is a no-op for a face that was already answered, so a
        // rejected proposal cannot be resurrected by a later matcher run.
        let mut statement = transaction.prepare_cached(
            "INSERT OR REPLACE INTO face_candidates (
                    observation_id, person_id, matcher_fingerprint,
                    similarity_micro, state
                 )
                 SELECT ?1, ?2, ?3, ?4, 'pending'
                 WHERE NOT EXISTS (
                    SELECT 1 FROM face_decisions WHERE observation_id = ?1
                 )",
        )?;
        for candidate in candidates {
            statement.execute(params![
                candidate.observation_id,
                candidate.person_id,
                matcher_fingerprint,
                to_micro(candidate.similarity),
            ])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_domain::{FaceReviewFilter, FaceReviewState};
    use std::path::PathBuf;

    const DETECTOR: &str = "scrfd/abc/detect-v1";
    const EMBEDDER: &str = "adaface/def/align-v1";

    fn rect(x: f32, y: f32, width: f32, height: f32) -> NormalizedRect {
        NormalizedRect::new(x, y, width, height)
    }

    fn face(id: &str, index: u32, bbox: NormalizedRect, revision: &str) -> StoredFace {
        StoredFace {
            observation: FaceObservation {
                observation_id: id.into(),
                asset_id: "asset-1".into(),
                asset_path: PathBuf::from("/photos/a.jpg"),
                source_revision: revision.into(),
                local_index: index,
                bbox,
                landmarks: vec![NormalizedPoint::new(0.2, 0.2); 5],
                detection_score: 0.97,
                detector_fingerprint: DETECTOR.into(),
            },
            embedding: vec![0.5, -0.25, 0.125],
            face_pixels: 128,
            clarity: 0.8,
        }
    }

    fn store(library: &Library, faces: &[StoredFace], revision: &str) {
        library
            .replace_asset_faces(
                Path::new("/photos/a.jpg"),
                "asset-1",
                revision,
                DETECTOR,
                EMBEDDER,
                faces,
            )
            .unwrap();
    }

    #[test]
    fn confirmed_person_tags_filter_by_ancestor_and_withdraw_on_rejection() {
        let library = Library::in_memory().unwrap();
        let tag = library.ensure_custom_tag_path("人物|家人|Alice").unwrap();
        assert_eq!(
            library
                .ensure_custom_tag_path("人物|家人|alice")
                .unwrap()
                .id,
            tag.id
        );
        library
            .create_person("alice", "Alice", Some(tag.id))
            .unwrap();
        store(
            &library,
            &[face("face", 0, rect(0.1, 0.1, 0.2, 0.2), "rev")],
            "rev",
        );
        let asset = oxy_domain::AssetSummary {
            id: "asset-1".into(),
            path: "/photos/a.jpg".into(),
            name: "a.jpg".into(),
            extension: "jpg".into(),
            kind: oxy_domain::AssetKind::Jpeg,
            size_bytes: 1,
            modified_at_ms: 0,
            has_sidecar: false,
            rating: None,
            color_label: None,
            pick_label: None,
        };
        let query = oxy_domain::AssetQuery {
            tag_ids: vec![tag.parent_id.unwrap()],
            ..Default::default()
        };
        assert!(
            library
                .filter_assets_by_tags(std::slice::from_ref(&asset), &query)
                .unwrap()
                .is_empty()
        );
        library
            .record_face_decision(
                "face",
                &FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
                None,
            )
            .unwrap();
        library.refresh_person_tag_projection().unwrap();
        assert_eq!(
            library
                .filter_assets_by_tags(std::slice::from_ref(&asset), &query)
                .unwrap()
                .len(),
            1
        );
        library
            .record_face_decision("face", &FaceDecision::NotFace, None)
            .unwrap();
        library.refresh_person_tag_projection().unwrap();
        assert!(
            library
                .filter_assets_by_tags(&[asset], &query)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn linked_person_tags_preserve_manual_ownership() {
        let library = Library::in_memory().unwrap();
        let path = Path::new("/photos/a.jpg");
        let tag = library.create_custom_tag(None, "Alice").unwrap();
        library
            .create_person("alice", "Alice", Some(tag.id))
            .unwrap();
        store(
            &library,
            &[face("face", 0, rect(0.1, 0.1, 0.2, 0.2), "rev")],
            "rev",
        );
        library
            .record_face_decision(
                "face",
                &FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
                None,
            )
            .unwrap();
        assert!(library.refresh_person_tag_projection().unwrap());
        assert!(!library.refresh_person_tag_projection().unwrap());
        library.set_asset_tag(&[path.into()], tag.id, true).unwrap();
        library.clear_face_decision("face").unwrap();
        assert!(library.refresh_person_tag_projection().unwrap());
        let count: i64 = library
            .connection
            .lock()
            .query_row("SELECT COUNT(*) FROM asset_tags", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "manual assignment survives withdrawal of face confirmation"
        );
    }

    #[test]
    fn projection_admission_rejects_old_workers_and_invalidation() {
        let library = Library::in_memory().unwrap();
        let path = Path::new("/photo.jpg");
        let old = library.begin_face_analysis(path, "revision-a").unwrap();
        let new = library.begin_face_analysis(path, "revision-b").unwrap();
        let publish = |revision, valid_at| {
            library.accept_face_result(FaceResultWrite {
                asset_path: path,
                asset_id: "photo",
                source_revision: revision,
                detector_fingerprint: "detector",
                embedder_fingerprint: "embedder",
                faces: &[],
                crops: &[],
                valid_at: Some(valid_at),
                cancellation: None,
            })
        };
        assert!(matches!(
            publish("revision-a", old),
            Err(LibraryError::StaleFaceResult)
        ));
        publish("revision-b", new).unwrap();
        assert!(
            library
                .face_scan_is_current(path, "revision-b", "detector")
                .unwrap()
        );
        assert!(
            library
                .face_models_are_current(path, "revision-b", "detector", "embedder")
                .unwrap()
        );
        assert!(
            !library
                .face_models_are_current(path, "revision-b", "detector", "replacement-embedder")
                .unwrap()
        );
        let invalidated = library.begin_face_analysis(path, "revision-b").unwrap();
        library.invalidate_face_projections().unwrap();
        assert!(matches!(
            publish("revision-b", invalidated),
            Err(LibraryError::StaleFaceResult)
        ));
        assert!(
            !library
                .face_scan_is_current(path, "revision-b", "detector")
                .unwrap()
        );
    }

    #[test]
    fn hundred_thousand_targets_are_bounded_and_deduplicated() {
        let library = Library::in_memory().unwrap();
        library.connection.lock().execute_batch(
            "INSERT INTO library_roots(path) VALUES('/photos'),('/photos/nested');
             WITH RECURSIVE seq(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM seq WHERE x<100000)
             INSERT INTO indexed_assets(root_path,path,parent_path,id,name,extension,kind,modified_at_ms,size_bytes,has_sidecar,scan_id)
             SELECT '/photos', '/photos/nested/' || printf('%06d.jpg',x), '/photos/nested', CAST(x AS TEXT), 'photo', 'jpg','jpeg',1,1,0,1 FROM seq;
             INSERT INTO indexed_assets SELECT '/photos/nested',path,parent_path,id,name,extension,kind,modified_at_ms,size_bytes,has_sidecar,scan_id FROM indexed_assets;
             INSERT INTO indexed_roots(root_path,asset_count,directory_count) VALUES('/photos',100000,1);"
        ).unwrap();
        let request = oxy_domain::FaceAnalysisRequest::library(false);
        let mut cursor = None;
        let mut count = 0;
        loop {
            let page = library
                .face_analysis_page(&request, cursor.as_deref())
                .unwrap();
            assert!(page.len() <= 256);
            if page.is_empty() {
                break;
            }
            assert!(page.windows(2).all(|pair| pair[0].path < pair[1].path));
            count += page.len();
            cursor = page.last().map(|target| target.path.clone());
        }
        assert_eq!(count, 100000);
        let run = library.resume_face_run("test").unwrap();
        library
            .checkpoint_face_run("test", run.generation, cursor.as_ref().unwrap())
            .unwrap();
        assert_eq!(library.resume_face_run("test").unwrap().cursor, cursor);
        library
            .connection
            .lock()
            .execute("UPDATE indexed_roots SET asset_count=100001", [])
            .unwrap();
        assert!(!library.finish_face_run("test", run.generation).unwrap());
        let fresh = library.resume_face_run("test").unwrap();
        assert!(fresh.generation > run.generation);
        assert!(fresh.cursor.is_none());
    }

    #[test]
    fn legacy_crop_schema_migrates_before_saving_detected_faces() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("library.sqlite");
        let library = Library::open(&database).unwrap();
        store(
            &library,
            &[face("old", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1")],
            "rev-1",
        );
        library.connection.lock().execute_batch(
            "DROP TABLE face_crops;
             CREATE TABLE face_crops (
                 observation_id TEXT NOT NULL REFERENCES face_observations(observation_id) ON DELETE CASCADE,
                 size INTEGER NOT NULL, jpeg BLOB NOT NULL,
                 PRIMARY KEY(observation_id, size)
             );
             INSERT INTO face_crops VALUES('old', 128, X'010203');",
        ).unwrap();
        drop(library);

        let library = Library::open(&database).unwrap();
        assert_eq!(library.face_crop("old", 96).unwrap(), None);
        let count: i64 = library
            .connection
            .lock()
            .query_row(
                "SELECT count(*) FROM face_observations WHERE observation_id='old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let path = Path::new("/photos/a.jpg");
        let valid_at = library.begin_face_analysis(path, "rev-2").unwrap();
        library
            .accept_face_result(FaceResultWrite {
                asset_path: path,
                asset_id: "asset-1",
                source_revision: "rev-2",
                detector_fingerprint: DETECTOR,
                embedder_fingerprint: EMBEDDER,
                faces: &[face("new", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-2")],
                crops: &[StoredFaceCrop {
                    observation_id: "new".into(),
                    size: 128,
                    jpeg: vec![4, 5, 6],
                }],
                valid_at: Some(valid_at),
                cancellation: None,
            })
            .unwrap();
        assert!(
            library
                .face_models_are_current(path, "rev-2", DETECTOR, EMBEDDER)
                .unwrap()
        );
        library
            .connection
            .lock()
            .execute(
                "INSERT INTO face_crops VALUES('new', 128, 'other-policy', X'070809')",
                [],
            )
            .unwrap();
        drop(library);
        let library = Library::open(&database).unwrap();
        assert_eq!(
            library.face_crop("new", 96).unwrap(),
            Some((128, vec![4, 5, 6]))
        );
    }

    #[test]
    fn scan_crops_are_reused_and_retired_with_the_observation() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1")],
            "rev-1",
        );
        library
            .store_face_crops(&[StoredFaceCrop {
                observation_id: "f1".into(),
                size: 128,
                jpeg: vec![1, 2, 3],
            }])
            .unwrap();
        assert_eq!(
            library.face_crop("f1", 96).unwrap(),
            Some((128, vec![1, 2, 3]))
        );
        assert_eq!(library.face_crop("f1", 256).unwrap(), None);

        store(
            &library,
            &[face("f2", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-2")],
            "rev-2",
        );
        assert_eq!(library.face_crop("f1", 96).unwrap(), None);
    }

    #[test]
    fn stores_and_reads_observations_with_embeddings() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[
                face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1"),
                face("f2", 1, rect(0.5, 0.5, 0.2, 0.3), "rev-1"),
            ],
            "rev-1",
        );

        let observations = library
            .face_observations_for_asset(Path::new("/photos/a.jpg"))
            .unwrap();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].observation_id, "f1");
        // Micro-unit storage must round-trip exactly enough for a UI overlay.
        assert!((observations[1].bbox.x - 0.5).abs() < 1e-6);
        assert!((observations[1].bbox.height - 0.3).abs() < 1e-6);

        let embeddings = library.undecided_face_embeddings(EMBEDDER, 100).unwrap();
        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0].1, vec![0.5, -0.25, 0.125]);
        let cluster_embeddings = library
            .undecided_face_cluster_embeddings(EMBEDDER, 100)
            .unwrap();
        assert_eq!(cluster_embeddings.len(), 2);
        assert_eq!(cluster_embeddings[0].1, "asset-1");
        assert_eq!(cluster_embeddings[0].2, vec![0.5, -0.25, 0.125]);

        assert_eq!(
            library
                .asset_face_fingerprint(Path::new("/photos/a.jpg"))
                .unwrap()
                .as_deref(),
            Some(DETECTOR)
        );
    }

    #[test]
    fn reanalysis_keeps_decisions_and_rebinds_them_by_overlap() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[face("f1", 0, rect(0.10, 0.10, 0.20, 0.20), "rev-1")],
            "rev-1",
        );
        library.create_person("p-1", "Alice", None).unwrap();
        library
            .record_face_decision(
                "f1",
                &FaceDecision::ConfirmPerson {
                    person_id: "p-1".into(),
                },
                Some(0.62),
            )
            .unwrap();

        // A new detector moves the box slightly and renames the observation.
        store(
            &library,
            &[face("f2", 0, rect(0.105, 0.098, 0.205, 0.198), "rev-1")],
            "rev-1",
        );

        let decisions = library.face_decisions().unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(
            decisions[0].observation_id.as_deref(),
            Some("f2"),
            "the decision must follow the face, not the detector's id"
        );
        assert_eq!(
            decisions[0].decision,
            FaceDecision::ConfirmPerson {
                person_id: "p-1".into()
            }
        );

        let page = library
            .face_review_page(FaceReviewFilter::Confirmed, 0, 10)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].state, FaceReviewState::Confirmed);
        assert_eq!(
            page.items[0].confirmed_person_name.as_deref(),
            Some("Alice")
        );
    }

    #[test]
    fn a_decision_survives_even_when_its_face_disappears() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[face("f1", 0, rect(0.10, 0.10, 0.20, 0.20), "rev-1")],
            "rev-1",
        );
        library
            .record_face_decision("f1", &FaceDecision::NotFace, None)
            .unwrap();

        // The face is gone from the new analysis entirely.
        store(
            &library,
            &[face("f9", 0, rect(0.70, 0.70, 0.10, 0.10), "rev-1")],
            "rev-1",
        );

        let decisions = library.face_decisions().unwrap();
        assert_eq!(decisions.len(), 1, "user data must not be deleted");
        assert_eq!(decisions[0].decision, FaceDecision::NotFace);
        assert_eq!(
            decisions[0].observation_id, None,
            "an unmatched decision keeps its region and loses its link"
        );
        // The unrelated new face is still unknown, not suppressed.
        let page = library
            .face_review_page(FaceReviewFilter::Unknown, 0, 10)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].observation_id, "f9");
    }

    #[test]
    fn not_face_suppresses_a_redetected_face() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[face("f1", 0, rect(0.10, 0.10, 0.20, 0.20), "rev-1")],
            "rev-1",
        );
        library
            .record_face_decision("f1", &FaceDecision::NotFace, None)
            .unwrap();
        store(
            &library,
            &[face("f2", 0, rect(0.10, 0.10, 0.20, 0.20), "rev-1")],
            "rev-1",
        );

        // The same detector must not recreate a review item for a region the
        // user already dismissed.
        let unknown = library
            .face_review_page(FaceReviewFilter::Unknown, 0, 10)
            .unwrap();
        assert!(unknown.items.is_empty());
        let rejected = library
            .face_review_page(FaceReviewFilter::Rejected, 0, 10)
            .unwrap();
        assert_eq!(rejected.items.len(), 1);
        assert_eq!(rejected.items[0].state, FaceReviewState::NotFace);
    }

    #[test]
    fn confirmed_faces_are_excluded_from_clustering_input() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[
                face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1"),
                face("f2", 1, rect(0.4, 0.1, 0.2, 0.2), "rev-1"),
                face("f3", 2, rect(0.7, 0.1, 0.2, 0.2), "rev-1"),
            ],
            "rev-1",
        );
        library.create_person("p-1", "Alice", None).unwrap();
        library
            .record_face_decision(
                "f1",
                &FaceDecision::ConfirmPerson {
                    person_id: "p-1".into(),
                },
                Some(0.62),
            )
            .unwrap();
        library
            .record_face_decision("f2", &FaceDecision::NotFace, None)
            .unwrap();

        let embeddings = library.undecided_face_embeddings(EMBEDDER, 100).unwrap();
        assert_eq!(embeddings.len(), 1);
        assert_eq!(embeddings[0].0, "f3");
    }

    #[test]
    fn a_candidate_is_pending_until_the_user_answers() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1")],
            "rev-1",
        );
        library.create_person("p-1", "Alice", None).unwrap();
        library
            .replace_face_candidates(
                "matcher-v1",
                &[FaceCandidate {
                    observation_id: "f1".into(),
                    person_id: "p-1".into(),
                    person_name: "Alice".into(),
                    similarity: 0.62,
                    matcher_fingerprint: "matcher-v1".into(),
                }],
            )
            .unwrap();

        let page = library
            .face_review_page(FaceReviewFilter::Pending, 0, 10)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].state, FaceReviewState::Pending);
        let candidate = page.items[0].candidate.as_ref().unwrap();
        assert_eq!(candidate.person_id, "p-1");
        assert!((candidate.similarity - 0.62).abs() < 1e-6);

        // Rejecting the proposal is a user fact: the cached candidate is gone
        // and the durable rejection keeps it from returning.
        library
            .record_face_decision(
                "f1",
                &FaceDecision::RejectPerson {
                    person_id: "p-1".into(),
                },
                Some(0.41),
            )
            .unwrap();
        library
            .replace_face_candidates(
                "matcher-v1",
                &[FaceCandidate {
                    observation_id: "f1".into(),
                    person_id: "p-1".into(),
                    person_name: "Alice".into(),
                    similarity: 0.99,
                    matcher_fingerprint: "matcher-v1".into(),
                }],
            )
            .unwrap();
        let page = library
            .face_review_page(FaceReviewFilter::Pending, 0, 10)
            .unwrap();
        assert!(page.items.is_empty());
        let rejected = library
            .face_review_page(FaceReviewFilter::Rejected, 0, 10)
            .unwrap();
        assert_eq!(rejected.items.len(), 1);
        assert_eq!(rejected.items[0].state, FaceReviewState::Rejected);
    }

    #[test]
    fn the_unreviewed_queue_is_every_face_without_an_answer() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[
                face("f1", 0, rect(0.10, 0.10, 0.10, 0.10), "rev-1"),
                face("f2", 1, rect(0.30, 0.10, 0.10, 0.10), "rev-1"),
                face("f3", 2, rect(0.50, 0.10, 0.10, 0.10), "rev-1"),
                face("f4", 3, rect(0.70, 0.10, 0.10, 0.10), "rev-1"),
            ],
            "rev-1",
        );
        library.create_person("p-1", "Alice", None).unwrap();
        // Only f2 has a matcher proposal; the rest are simply unknown faces.
        library
            .replace_face_candidates(
                "matcher-v1",
                &[FaceCandidate {
                    observation_id: "f2".into(),
                    person_id: "p-1".into(),
                    person_name: "Alice".into(),
                    similarity: 0.62,
                    matcher_fingerprint: "matcher-v1".into(),
                }],
            )
            .unwrap();
        library
            .record_face_decision(
                "f3",
                &FaceDecision::ConfirmPerson {
                    person_id: "p-1".into(),
                },
                None,
            )
            .unwrap();
        library
            .record_face_decision("f4", &FaceDecision::NotFace, None)
            .unwrap();

        // A library with no named people has no proposals at all, so the queue a
        // first pass works through must be "no answer yet", not "has a
        // proposal": this is the difference between an empty list and 990 faces.
        let unreviewed = library
            .face_review_page(FaceReviewFilter::Unreviewed, 0, 10)
            .unwrap();
        let ids: Vec<&str> = unreviewed
            .items
            .iter()
            .map(|item| item.observation_id.as_str())
            .collect();
        assert_eq!(ids, vec!["f1", "f2"]);
        assert_eq!(unreviewed.total, 2);

        // The narrower filters still separate the two reasons a face is waiting.
        assert_eq!(
            library
                .face_review_page(FaceReviewFilter::Pending, 0, 10)
                .unwrap()
                .items
                .len(),
            1
        );
        assert_eq!(
            library
                .face_review_page(FaceReviewFilter::Unknown, 0, 10)
                .unwrap()
                .items
                .len(),
            1
        );
    }

    #[test]
    fn review_paging_respects_the_filter_before_applying_the_limit() {
        let library = Library::in_memory().unwrap();
        let faces: Vec<StoredFace> = (0..5)
            .map(|index| {
                face(
                    &format!("f{index}"),
                    index,
                    rect(0.05 * index as f32, 0.1, 0.05, 0.05),
                    "rev-1",
                )
            })
            .collect();
        store(&library, &faces, "rev-1");
        library.create_person("p-1", "Alice", None).unwrap();
        library
            .record_face_decision(
                "f0",
                &FaceDecision::ConfirmPerson {
                    person_id: "p-1".into(),
                },
                Some(0.62),
            )
            .unwrap();

        let first = library
            .face_review_page(FaceReviewFilter::Unknown, 0, 2)
            .unwrap();
        assert_eq!(first.total, 4);
        assert_eq!(first.items.len(), 2);
        let second = library
            .face_review_page(FaceReviewFilter::Unknown, 2, 2)
            .unwrap();
        assert_eq!(second.items.len(), 2);
        assert_eq!(second.next_cursor, None);
        assert_ne!(
            first.items[0].observation_id,
            second.items[0].observation_id
        );
    }

    #[test]
    fn review_quality_filter_is_applied_before_paging() {
        let library = Library::in_memory().unwrap();
        let mut blurry = face("blurry", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1");
        blurry.clarity = 0.2;
        blurry.face_pixels = 40;
        let sharp = face("sharp", 1, rect(0.4, 0.1, 0.2, 0.2), "rev-1");
        store(&library, &[blurry, sharp], "rev-1");

        let page = library
            .face_review_page_filtered(FaceReviewFilter::Unknown, 0, 1, 0.9, 64, 0.5)
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].observation_id, "sharp");
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn person_rename_does_not_touch_confirmations() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1")],
            "rev-1",
        );
        library.create_person("p-1", "Alica", None).unwrap();
        library
            .record_face_decision(
                "f1",
                &FaceDecision::ConfirmPerson {
                    person_id: "p-1".into(),
                },
                Some(0.62),
            )
            .unwrap();
        library.rename_person("p-1", "Alice Zhang").unwrap();

        let persons = library.persons().unwrap();
        assert_eq!(persons.len(), 1);
        assert_eq!(persons[0].display_name, "Alice Zhang");
        assert_eq!(persons[0].face_count, 1);
        assert_eq!(persons[0].cover_observation_id.as_deref(), Some("f1"));

        let decisions = library.face_decisions().unwrap();
        assert_eq!(
            decisions[0].decision,
            FaceDecision::ConfirmPerson {
                person_id: "p-1".into()
            }
        );
        assert!(matches!(
            library.rename_person("p-1", "   "),
            Err(LibraryError::InvalidPersonName)
        ));
        assert!(matches!(
            library.rename_person("missing", "x"),
            Err(LibraryError::MissingPerson(_))
        ));
    }

    #[test]
    fn deleting_a_person_removes_its_decisions_but_keeps_not_face() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[
                face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1"),
                face("f2", 1, rect(0.5, 0.1, 0.2, 0.2), "rev-1"),
            ],
            "rev-1",
        );
        library.create_person("p-1", "Alice", None).unwrap();
        library
            .record_face_decision(
                "f1",
                &FaceDecision::ConfirmPerson {
                    person_id: "p-1".into(),
                },
                Some(0.62),
            )
            .unwrap();
        library
            .record_face_decision("f2", &FaceDecision::NotFace, None)
            .unwrap();

        let removed = library.delete_person("p-1").unwrap();
        assert_eq!(removed, 1);
        let decisions = library.face_decisions().unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].decision, FaceDecision::NotFace);
        assert!(library.persons().unwrap().is_empty());
    }

    #[test]
    fn decisions_require_a_real_observation_and_person() {
        let library = Library::in_memory().unwrap();
        assert!(matches!(
            library.record_face_decision("missing", &FaceDecision::NotFace, None),
            Err(LibraryError::MissingFaceObservation(_))
        ));
        store(
            &library,
            &[face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1")],
            "rev-1",
        );
        assert!(matches!(
            library.record_face_decision(
                "f1",
                &FaceDecision::ConfirmPerson {
                    person_id: "ghost".into()
                },
                None
            ),
            Err(LibraryError::MissingPerson(_))
        ));
    }

    #[test]
    fn the_proposed_similarity_round_trips_through_the_projection() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1")],
            "rev-1",
        );
        library.create_person("p-1", "Alice", None).unwrap();
        library
            .replace_face_candidates(
                "matcher-v1",
                &[FaceCandidate {
                    observation_id: "f1".into(),
                    person_id: "p-1".into(),
                    person_name: "Alice".into(),
                    similarity: 0.512,
                    matcher_fingerprint: "matcher-v1".into(),
                }],
            )
            .unwrap();
        let candidate = library.pending_candidate_for("f1").unwrap().unwrap();
        assert!((candidate.similarity - 0.512).abs() < 1e-6);

        library
            .record_face_decision(
                "f1",
                &FaceDecision::ConfirmPerson {
                    person_id: "p-1".into(),
                },
                Some(candidate.similarity),
            )
            .unwrap();

        // The score must survive the cache so a migration into the durable
        // store does not silently drop the calibration history.
        let (_, decisions) = library.export_user_data().unwrap();
        assert_eq!(decisions.len(), 1);
        let stored = decisions[0].proposed_similarity.expect("score kept");
        assert!((stored - 0.512).abs() < 1e-6);

        // And it must survive a projection rebuild in the other direction.
        library
            .replace_user_data(
                &library
                    .persons()
                    .unwrap()
                    .iter()
                    .map(Into::into)
                    .collect::<Vec<_>>(),
                &decisions,
            )
            .unwrap();
        let (_, reparsed) = library.export_user_data().unwrap();
        assert!((reparsed[0].proposed_similarity.unwrap() - 0.512).abs() < 1e-6);
    }

    #[test]
    fn clearing_a_decision_returns_the_face_to_the_queue() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1")],
            "rev-1",
        );
        library
            .record_face_decision("f1", &FaceDecision::NotFace, None)
            .unwrap();
        library.clear_face_decision("f1").unwrap();

        assert!(library.face_decisions().unwrap().is_empty());
        let unknown = library
            .face_review_page(FaceReviewFilter::Unknown, 0, 10)
            .unwrap();
        assert_eq!(unknown.items.len(), 1);
    }

    #[test]
    fn decision_history_is_appended_newest_first() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1")],
            "rev-1",
        );
        library.create_person("p-1", "Alice", None).unwrap();
        library
            .record_face_decision("f1", &FaceDecision::NotFace, None)
            .unwrap();
        library
            .record_face_decision(
                "f1",
                &FaceDecision::ConfirmPerson {
                    person_id: "p-1".into(),
                },
                Some(0.62),
            )
            .unwrap();

        let records = library.face_decision_records(10).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0].decision,
            FaceDecision::ConfirmPerson {
                person_id: "p-1".into()
            }
        );
        assert_eq!(records[1].decision, FaceDecision::NotFace);
        assert!((records[0].region.width - 0.2).abs() < 1e-6);
    }

    #[test]
    fn clusters_round_trip_per_fingerprint() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[
                face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1"),
                face("f2", 1, rect(0.4, 0.1, 0.2, 0.2), "rev-1"),
            ],
            "rev-1",
        );
        library
            .replace_face_clusters(
                "cluster-v1",
                &[FaceCluster {
                    cluster_id: "cluster-1".into(),
                    observation_ids: vec!["f1".into(), "f2".into()],
                    representative_observation_id: "f1".into(),
                    member_count: 2,
                    cohesion: 0.82,
                    outlier_observation_ids: vec!["f2".into()],
                    suggested_person_id: None,
                    suggested_name: None,
                    member_quality: Vec::new(),
                }],
            )
            .unwrap();

        let clusters = library.face_clusters("cluster-v1").unwrap();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].observation_ids, vec!["f1", "f2"]);
        assert_eq!(clusters[0].outlier_observation_ids, vec!["f2"]);
        assert_eq!(clusters[0].member_quality.len(), 2);
        assert!(
            clusters[0].member_quality.iter().all(|quality| {
                quality.face_pixels == 128 && (quality.clarity - 0.8).abs() < 1e-6
            })
        );
        assert!((clusters[0].cohesion - 0.82).abs() < 1e-6);
        assert!(library.face_clusters("other").unwrap().is_empty());
    }

    #[test]
    fn settings_round_trip_and_are_sanitized() {
        let library = Library::in_memory().unwrap();
        assert_eq!(
            library.face_analyzer_settings().unwrap(),
            FaceAnalyzerSettings::default().sanitized()
        );

        let stored = library
            .update_face_analyzer_settings(&FaceAnalyzerSettings {
                detection_confidence: 5.0,
                match_threshold: 0.4,
                ..FaceAnalyzerSettings::default()
            })
            .unwrap();
        assert_eq!(stored.detection_confidence, 0.99);
        assert_eq!(
            library
                .face_analyzer_settings()
                .unwrap()
                .detection_confidence,
            0.99
        );
    }

    #[test]
    fn stats_summarize_pending_unknown_and_persons() {
        let library = Library::in_memory().unwrap();
        store(
            &library,
            &[
                face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1"),
                face("f2", 1, rect(0.4, 0.1, 0.2, 0.2), "rev-1"),
                face("f3", 2, rect(0.7, 0.1, 0.2, 0.2), "rev-1"),
            ],
            "rev-1",
        );
        library.create_person("p-1", "Alice", None).unwrap();
        library
            .replace_face_candidates(
                "matcher-v1",
                &[FaceCandidate {
                    observation_id: "f1".into(),
                    person_id: "p-1".into(),
                    person_name: "Alice".into(),
                    similarity: 0.5,
                    matcher_fingerprint: "matcher-v1".into(),
                }],
            )
            .unwrap();
        library
            .record_face_decision("f3", &FaceDecision::NotFace, None)
            .unwrap();

        let stats = library.face_library_stats().unwrap();
        assert_eq!(stats.analyzed_assets, 1);
        assert_eq!(stats.faces_detected, 3);
        assert_eq!(stats.persons, 1);
        assert_eq!(stats.pending_reviews, 1);
        assert_eq!(stats.unknown_faces, 1);
    }

    #[test]
    fn analysis_targets_resume_and_skip_assets_without_faces() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a.jpg"), b"aaaa").unwrap();
        std::fs::write(directory.path().join("b.jpg"), b"bbbb").unwrap();
        let library = Library::in_memory().unwrap();
        library.add_root(directory.path()).unwrap();
        library.index_root(directory.path()).unwrap();

        let targets = library.face_analysis_targets(DETECTOR, false, 100).unwrap();
        assert_eq!(targets.len(), 2);
        let (analyzed, total) = library.face_analysis_counts(DETECTOR).unwrap();
        assert_eq!((analyzed, total), (0, 2));

        let a = targets
            .iter()
            .find(|target| target.path.ends_with("a.jpg"))
            .unwrap()
            .clone();
        let b = targets
            .iter()
            .find(|target| target.path.ends_with("b.jpg"))
            .unwrap()
            .clone();

        // a.jpg has one face, b.jpg has none. Both must be checkpointed so the
        // empty one is not decoded again on every run.
        library
            .replace_asset_faces(
                &a.path,
                "asset-a",
                &a.source_revision,
                DETECTOR,
                EMBEDDER,
                &[StoredFace {
                    observation: FaceObservation {
                        observation_id: "f1".into(),
                        asset_id: "asset-a".into(),
                        asset_path: a.path.clone(),
                        source_revision: a.source_revision.clone(),
                        local_index: 0,
                        bbox: rect(0.1, 0.1, 0.2, 0.2),
                        landmarks: Vec::new(),
                        detection_score: 0.9,
                        detector_fingerprint: DETECTOR.into(),
                    },
                    embedding: vec![1.0, 0.0],
                    face_pixels: 128,
                    clarity: 0.8,
                }],
            )
            .unwrap();
        library
            .replace_asset_faces(
                &b.path,
                "asset-b",
                &b.source_revision,
                DETECTOR,
                EMBEDDER,
                &[],
            )
            .unwrap();

        assert!(
            library
                .face_analysis_targets(DETECTOR, false, 100)
                .unwrap()
                .is_empty(),
            "a completed scan is a checkpoint, including a zero-face one"
        );
        let (analyzed, total) = library.face_analysis_counts(DETECTOR).unwrap();
        assert_eq!((analyzed, total), (2, 2));

        // Forcing re-analysis ignores the checkpoint but not the index.
        assert_eq!(
            library
                .face_analysis_targets(DETECTOR, true, 100)
                .unwrap()
                .len(),
            2
        );
        // A different detector also invalidates the checkpoint.
        assert_eq!(
            library
                .face_analysis_targets("scrfd/other/detect-v2", false, 100)
                .unwrap()
                .len(),
            2
        );

        // Changing the source bytes schedules exactly that asset again.
        std::fs::write(directory.path().join("a.jpg"), b"aaaa-changed").unwrap();
        library.index_root(directory.path()).unwrap();
        let targets = library.face_analysis_targets(DETECTOR, false, 100).unwrap();
        assert_eq!(targets.len(), 1);
        assert!(targets[0].path.ends_with("a.jpg"));
    }

    #[test]
    fn directory_scoped_targets_cover_every_page_but_not_the_whole_library() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        for name in ["a.jpg", "b.jpg", "c.jpg"] {
            std::fs::write(first.join(name), b"jpeg").unwrap();
        }
        std::fs::write(second.join("d.jpg"), b"jpeg").unwrap();

        let library = Library::in_memory().unwrap();
        library.add_root(directory.path()).unwrap();
        library.index_root(directory.path()).unwrap();

        // The index stores canonical paths, so a caller must pass the same form
        // the session uses; `tempdir` returns a symlinked path on macOS.
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let first_canonical = std::fs::canonicalize(&first).unwrap();
        let scoped = library
            .face_analysis_targets_in_directory(&root, &first_canonical, DETECTOR, false, 100)
            .unwrap();
        assert_eq!(
            scoped.len(),
            3,
            "every asset in the directory, not only a loaded page"
        );
        assert!(
            scoped
                .iter()
                .all(|target| target.path.parent() == Some(first_canonical.as_path()))
        );
        assert!(
            scoped.iter().all(|target| target.kind == AssetKind::Jpeg),
            "kind comes from the index"
        );

        // A limit is still honoured, so a huge directory can be paged.
        assert_eq!(
            library
                .face_analysis_targets_in_directory(&root, &first_canonical, DETECTOR, false, 2)
                .unwrap()
                .len(),
            2
        );
        // A different root does not leak into the scope.
        assert!(
            library
                .face_analysis_targets_in_directory(
                    &std::fs::canonicalize(&second).unwrap(),
                    &first_canonical,
                    DETECTOR,
                    false,
                    100
                )
                .unwrap()
                .is_empty()
        );
        // An unknown directory is simply empty, not an error.
        assert!(
            library
                .face_analysis_targets_in_directory(
                    &root,
                    &root.join("absent"),
                    DETECTOR,
                    false,
                    100
                )
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn explicit_paths_and_the_checkpoint_rule_agree() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a.jpg"), b"aaaa").unwrap();
        std::fs::write(directory.path().join("b.jpg"), b"bbbb").unwrap();
        let library = Library::in_memory().unwrap();
        library.add_root(directory.path()).unwrap();
        library.index_root(directory.path()).unwrap();

        let everything = library.face_analysis_targets(DETECTOR, false, 100).unwrap();
        assert_eq!(everything.len(), 2);

        // The path-scoped query must apply the same checkpoint predicate.
        let one = vec![everything[0].path.clone()];
        let scoped = library
            .face_analysis_targets_for_paths(&one, DETECTOR, false)
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].path, everything[0].path);
        assert_eq!(scoped[0].source_revision, everything[0].source_revision);

        library
            .replace_asset_faces(
                &everything[0].path,
                "asset",
                &everything[0].source_revision,
                DETECTOR,
                EMBEDDER,
                &[],
            )
            .unwrap();
        assert!(
            library
                .face_analysis_targets_for_paths(&one, DETECTOR, false)
                .unwrap()
                .is_empty(),
            "a completed checkpoint also skips a path-scoped target"
        );
        assert_eq!(
            library
                .face_analysis_targets_for_paths(&one, DETECTOR, true)
                .unwrap()
                .len(),
            1
        );
        assert!(
            library
                .face_analysis_targets_for_paths(&[], DETECTOR, false)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn decisions_persist_across_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("library.sqlite");
        let tag_id;
        {
            let library = Library::open(&database).unwrap();
            store(
                &library,
                &[face("f1", 0, rect(0.1, 0.1, 0.2, 0.2), "rev-1")],
                "rev-1",
            );
            let tag = library.create_custom_tag(None, "People").unwrap();
            tag_id = tag.id;
            library.create_person("p-1", "Alice", Some(tag.id)).unwrap();
            library
                .record_face_decision(
                    "f1",
                    &FaceDecision::ConfirmPerson {
                        person_id: "p-1".into(),
                    },
                    None,
                )
                .unwrap();
        }
        let library = Library::open(&database).unwrap();
        let persons = library.persons().unwrap();
        assert_eq!(persons.len(), 1);
        assert_eq!(persons[0].linked_tag_id, Some(tag_id));
        assert_eq!(persons[0].face_count, 1);
        assert_eq!(library.face_decisions().unwrap().len(), 1);
    }
}
