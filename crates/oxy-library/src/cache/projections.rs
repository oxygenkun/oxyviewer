//! Rebuildable metadata and image projections.
//!
//! A projection is an accepted observation about a file: it can always be
//! recomputed from the source. Ratings and labels stored here mirror the user's
//! XMP sidecars, but the sidecar remains the source of truth, so clearing this
//! table is a re-read, not a loss.

use crate::{Library, LibraryError};
use oxy_domain::{
    AssetSummary, ImageProjection, MetadataProjection, PickLabel, RenderLevel, ResourceLoadStatus,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::path::Path;

/// Tables created by [`ensure_schema`].
///
/// Declared beside the DDL so the registry cannot drift from the schema it
/// describes, and so claiming a table is the same act as creating it.
pub(super) const TABLES: &[&str] = &["resource_projection_sequence", "resource_projections"];

/// Tables [`clear`] empties of content but never of their allocator or format
/// marker; see [`crate::schema::preserved_on_clear`].
pub(super) const PRESERVED: &[&str] = &["resource_projection_sequence"];

pub(super) fn ensure_schema(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS resource_projection_sequence (
           id INTEGER PRIMARY KEY CHECK(id = 1),
           next_revision INTEGER NOT NULL
         );
         INSERT OR IGNORE INTO resource_projection_sequence(id, next_revision) VALUES (1, 1);
         CREATE TABLE IF NOT EXISTS resource_projections (
           path TEXT NOT NULL,
           parent_path TEXT NOT NULL,
           projection_kind TEXT NOT NULL,
           source_revision TEXT NOT NULL,
           valid_at INTEGER NOT NULL,
           state_revision INTEGER NOT NULL,
           status TEXT NOT NULL,
           rating INTEGER,
           color_label TEXT,
           pick_label TEXT,
           result_json TEXT,
           error TEXT,
           PRIMARY KEY(path, projection_kind)
         );
         CREATE INDEX IF NOT EXISTS resource_projections_parent
           ON resource_projections(parent_path);
         ",
    )?;
    let projection_columns = connection
        .prepare("PRAGMA table_info(resource_projections)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    let has_state_revision = projection_columns
        .iter()
        .any(|column| column == "state_revision");
    let has_legacy_projection_revision = projection_columns
        .iter()
        .any(|column| column == "projection_revision");
    if !has_state_revision && has_legacy_projection_revision {
        connection.execute(
            "ALTER TABLE resource_projections RENAME COLUMN projection_revision TO state_revision",
            [],
        )?;
    }
    let has_pick_label = projection_columns
        .iter()
        .any(|column| column == "pick_label");
    if !has_pick_label {
        connection.execute(
            "ALTER TABLE resource_projections ADD COLUMN pick_label TEXT",
            [],
        )?;
    }
    Ok(())
}

/// Empties every table created by [`ensure_schema`].
///
/// Kept next to the DDL it clears so a new projection table cannot be added
/// without deciding whether it is cleared here. The revision counter is left
/// alone; see [`crate::schema::preserved_on_clear`].
pub(super) fn clear(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch("DELETE FROM resource_projections")
}

impl Library {
    /// Allocates a revision in SQLite so observations from separate native
    /// processes share one comparable acceptance order.
    pub fn next_resource_revision(&self) -> Result<u64, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = next_resource_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    pub fn metadata_projection(
        &self,
        path: &Path,
        source_revision: &str,
    ) -> Result<Option<MetadataProjection>, LibraryError> {
        let connection = self.read_projection_connection();
        let projection = read_metadata_projection(&connection, path)?;
        Ok(projection.filter(|projection| projection.source_revision == source_revision))
    }

    /// Returns a stale-while-revalidate snapshot using only the source stat
    /// already present in an `AssetSummary`. Callers must still validate the
    /// complete revision, including sidecar or embedded metadata fingerprints.
    pub fn metadata_projection_for_asset(
        &self,
        asset: &AssetSummary,
    ) -> Result<Option<MetadataProjection>, LibraryError> {
        let connection = self.read_projection_connection();
        let prefix = format!("{}:{}:", asset.modified_at_ms, asset.size_bytes);
        Ok(
            read_metadata_projection(&connection, &asset.path)?.filter(|projection| {
                projection.status == ResourceLoadStatus::Ready
                    && projection.source_revision.starts_with(&prefix)
            }),
        )
    }

    /// Atomically accepts a metadata observation or returns the newer state
    /// that already won. The returned revision is the only revision sent to UI.
    pub fn accept_metadata_projection(
        &self,
        mut candidate: MetadataProjection,
    ) -> Result<MetadataProjection, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(current) = read_metadata_projection(&transaction, &candidate.path)?
            && current.valid_at > candidate.valid_at
        {
            transaction.commit()?;
            return Ok(current);
        }
        candidate.state_revision = next_resource_revision(&transaction)?;
        transaction.execute(
            "INSERT INTO resource_projections(
               path, parent_path, projection_kind, source_revision, valid_at,
               state_revision, status, rating, color_label, pick_label, result_json, error
             ) VALUES (?1, ?2, 'metadata', ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10)
             ON CONFLICT(path, projection_kind) DO UPDATE SET
               parent_path=excluded.parent_path,
               source_revision=excluded.source_revision,
               valid_at=excluded.valid_at,
               state_revision=excluded.state_revision,
               status=excluded.status,
               rating=excluded.rating,
               color_label=excluded.color_label,
               pick_label=excluded.pick_label,
               result_json=NULL,
               error=excluded.error",
            params![
                candidate.path.to_string_lossy(),
                parent_string(&candidate.path),
                candidate.source_revision,
                candidate.valid_at as i64,
                candidate.state_revision as i64,
                status_name(candidate.status),
                candidate.rating,
                candidate.color_label,
                candidate.pick_label.map(PickLabel::as_str),
                candidate.error,
            ],
        )?;
        transaction.commit()?;
        Ok(candidate)
    }

    pub fn image_projection(
        &self,
        path: &Path,
        level: RenderLevel,
        source_revision: &str,
    ) -> Result<Option<ImageProjection>, LibraryError> {
        let connection = self.read_projection_connection();
        let projection = read_image_projection(&connection, path, level)?;
        Ok(projection.filter(|projection| projection.source_revision == source_revision))
    }

    /// Atomically accepts an image artifact observation using the same
    /// transaction sequence as metadata projections.
    /// A nonzero state revision is an optimistic restore: it may replace a
    /// process-local descriptor only while that observed state is still current.
    pub fn accept_image_projection(
        &self,
        mut candidate: ImageProjection,
    ) -> Result<ImageProjection, LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(current) =
            read_image_projection(&transaction, &candidate.path, candidate.level)?
            && (current.valid_at > candidate.valid_at
                || (candidate.state_revision != 0
                    && current.state_revision > candidate.state_revision))
        {
            transaction.commit()?;
            return Ok(current);
        }
        candidate.state_revision = next_resource_revision(&transaction)?;
        // Resource descriptors identify entries in the current process registry.
        // Persist only restart-safe artifact facts; a caller restoring a managed
        // file must register it again in its own process.
        let persisted_result = candidate.result.as_ref().and_then(|result| {
            let restart_safe = matches!(
                result.persistence,
                Some(oxy_domain::MediaPersistence::Persisted)
                    | Some(oxy_domain::MediaPersistence::NotApplicable)
                    | None
            ) && result.path.is_file();
            restart_safe.then(|| {
                let mut result = result.clone();
                result.resource = None;
                result
            })
        });
        let result_json = persisted_result
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        transaction.execute(
            "INSERT INTO resource_projections(
               path, parent_path, projection_kind, source_revision, valid_at,
               state_revision, status, rating, color_label, pick_label, result_json, error
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, NULL, ?8, ?9)
             ON CONFLICT(path, projection_kind) DO UPDATE SET
               parent_path=excluded.parent_path,
               source_revision=excluded.source_revision,
               valid_at=excluded.valid_at,
               state_revision=excluded.state_revision,
               status=excluded.status,
               rating=NULL,
               color_label=NULL,
               pick_label=NULL,
               result_json=excluded.result_json,
               error=excluded.error",
            params![
                candidate.path.to_string_lossy(),
                parent_string(&candidate.path),
                image_projection_kind(candidate.level),
                candidate.source_revision,
                candidate.valid_at as i64,
                candidate.state_revision as i64,
                status_name(candidate.status),
                result_json,
                candidate.error,
            ],
        )?;
        transaction.commit()?;
        Ok(candidate)
    }

    pub fn invalidate_resource_projections(&self, directory: &Path) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = next_resource_revision(&transaction)?;
        transaction.execute(
            "UPDATE resource_projections SET
               source_revision = 'invalidated:' || CAST(?1 AS TEXT),
               valid_at = ?1,
               state_revision = ?1,
               status = 'error',
               rating = NULL,
               color_label = NULL,
               pick_label = NULL,
               result_json = NULL,
               error = 'invalidated'
             WHERE parent_path = ?2",
            params![revision as i64, directory.to_string_lossy()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn invalidate_image_projections(&self) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = next_resource_revision(&transaction)?;
        transaction.execute(
            "UPDATE resource_projections SET
               source_revision = 'invalidated:' || CAST(?1 AS TEXT),
               valid_at = ?1,
               state_revision = ?1,
               status = 'error',
               result_json = NULL,
               error = 'invalidated'
             WHERE projection_kind LIKE 'image:%'",
            params![revision as i64],
        )?;
        transaction.commit()?;
        Ok(())
    }
}
fn next_resource_revision(transaction: &Transaction<'_>) -> Result<u64, rusqlite::Error> {
    let revision = transaction.query_row(
        "SELECT next_revision FROM resource_projection_sequence WHERE id = 1",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    transaction.execute(
        "UPDATE resource_projection_sequence SET next_revision = next_revision + 1 WHERE id = 1",
        [],
    )?;
    Ok(revision.max(0) as u64)
}

fn read_metadata_projection(
    connection: &Connection,
    path: &Path,
) -> Result<Option<MetadataProjection>, LibraryError> {
    let row = connection
        .query_row(
            "SELECT source_revision, state_revision, valid_at, status,
                    rating, color_label, pick_label, error
             FROM resource_projections
             WHERE path = ?1 AND projection_kind = 'metadata'",
            params![path.to_string_lossy()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<u8>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(
            source_revision,
            state_revision,
            valid_at,
            status,
            rating,
            color_label,
            pick_label,
            error,
        )| {
            Ok(MetadataProjection {
                path: path.to_path_buf(),
                source_revision,
                state_revision: state_revision.max(0) as u64,
                valid_at: valid_at.max(0) as u64,
                status: parse_status(&status)?,
                rating,
                color_label,
                pick_label: parse_pick_label(pick_label)?,
                error,
            })
        },
    )
    .transpose()
}

fn read_image_projection(
    connection: &Connection,
    path: &Path,
    level: RenderLevel,
) -> Result<Option<ImageProjection>, LibraryError> {
    let row = connection
        .query_row(
            "SELECT source_revision, state_revision, valid_at, status,
                    result_json, error
             FROM resource_projections
             WHERE path = ?1 AND projection_kind = ?2",
            params![path.to_string_lossy(), image_projection_kind(level)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(source_revision, state_revision, valid_at, status, result_json, error)| {
            Ok(ImageProjection {
                path: path.to_path_buf(),
                source_revision,
                state_revision: state_revision.max(0) as u64,
                valid_at: valid_at.max(0) as u64,
                status: parse_status(&status)?,
                level,
                result: result_json
                    .as_deref()
                    // Cached artifacts may predate the current image-facts schema.
                    // Keep the ordering fence, but let admission rebuild an unreadable
                    // result instead of failing every request for this image.
                    .and_then(|json| serde_json::from_str(json).ok()),
                error,
            })
        },
    )
    .transpose()
}

fn parent_string(path: &Path) -> String {
    path.parent()
        .unwrap_or_else(|| Path::new(""))
        .to_string_lossy()
        .into_owned()
}

fn image_projection_kind(level: RenderLevel) -> &'static str {
    match level {
        RenderLevel::Thumbnail => "image:thumbnail",
        RenderLevel::Preview => "image:preview",
        RenderLevel::Full => "image:full",
    }
}

fn status_name(status: ResourceLoadStatus) -> &'static str {
    match status {
        ResourceLoadStatus::Loading => "loading",
        ResourceLoadStatus::Ready => "ready",
        ResourceLoadStatus::Error => "error",
    }
}

fn parse_status(status: &str) -> Result<ResourceLoadStatus, rusqlite::Error> {
    match status {
        "loading" => Ok(ResourceLoadStatus::Loading),
        "ready" => Ok(ResourceLoadStatus::Ready),
        "error" => Ok(ResourceLoadStatus::Error),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_pick_label(value: Option<String>) -> Result<Option<PickLabel>, rusqlite::Error> {
    value
        .map(|value| match value.as_str() {
            "rejected" => Ok(PickLabel::Rejected),
            "pending" => Ok(PickLabel::Pending),
            "accepted" => Ok(PickLabel::Accepted),
            _ => Err(rusqlite::Error::InvalidQuery),
        })
        .transpose()
}
