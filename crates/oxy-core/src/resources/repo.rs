//! Statements for rebuildable resource projections and their monotonic allocator.
use super::projections::ProjectionError;
use super::types::{ImageProjection, MetadataProjection, ResourceLoadStatus};
use oxy_media::types::{PickLabel, RenderLevel};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::Path;

pub(super) fn next_resource_revision(
    transaction: &Transaction<'_>,
) -> Result<u64, rusqlite::Error> {
    let revision = transaction.query_row(
        "SELECT next_revision FROM resource_projection_sequence WHERE id = 1",
        [],
        |row| row.get::<_, i64>("next_revision"),
    )?;
    transaction.execute(
        "UPDATE resource_projection_sequence SET next_revision = next_revision + 1 WHERE id = 1",
        [],
    )?;
    Ok(revision.max(0) as u64)
}

pub(super) fn read_metadata_projection(
    connection: &Connection,
    path: &Path,
) -> Result<Option<MetadataProjection>, ProjectionError> {
    let row = connection
        .query_row(
            "SELECT source_revision, state_revision, valid_at, status,
                    rating, color_label, pick_label, error
             FROM resource_projections
             WHERE path = ?1 AND projection_kind = 'metadata'",
            params![path.to_string_lossy()],
            |row| {
                Ok((
                    row.get::<_, String>("source_revision")?,
                    row.get::<_, i64>("state_revision")?,
                    row.get::<_, i64>("valid_at")?,
                    row.get::<_, String>("status")?,
                    row.get::<_, Option<u8>>("rating")?,
                    row.get::<_, Option<String>>("color_label")?,
                    row.get::<_, Option<String>>("pick_label")?,
                    row.get::<_, Option<String>>("error")?,
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

pub(super) fn read_image_projection(
    connection: &Connection,
    path: &Path,
    level: RenderLevel,
) -> Result<Option<ImageProjection>, ProjectionError> {
    let row = connection
        .query_row(
            "SELECT source_revision, state_revision, valid_at, status,
                    result_json, error
             FROM resource_projections
             WHERE path = ?1 AND projection_kind = ?2",
            params![path.to_string_lossy(), image_projection_kind(level)],
            |row| {
                Ok((
                    row.get::<_, String>("source_revision")?,
                    row.get::<_, i64>("state_revision")?,
                    row.get::<_, i64>("valid_at")?,
                    row.get::<_, String>("status")?,
                    row.get::<_, Option<String>>("result_json")?,
                    row.get::<_, Option<String>>("error")?,
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
                    // An unreadable derived artifact can always be rebuilt.
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

pub(super) fn invalidate_images(
    connection: &Connection,
    revision: u64,
) -> Result<(), rusqlite::Error> {
    connection.execute(
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
    Ok(())
}

pub(super) fn invalidate_directory(
    connection: &Connection,
    revision: u64,
    directory: &Path,
) -> Result<(), rusqlite::Error> {
    connection.execute(
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
    Ok(())
}

pub(super) fn write_image(
    connection: &Connection,
    candidate: &ImageProjection,
    result_json: Option<String>,
) -> Result<(), rusqlite::Error> {
    connection.execute(
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
    Ok(())
}

pub(super) fn write_metadata(
    connection: &Connection,
    candidate: &MetadataProjection,
) -> Result<(), rusqlite::Error> {
    connection.execute(
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
    Ok(())
}
