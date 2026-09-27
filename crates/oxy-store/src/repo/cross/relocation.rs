//! Statements for an explicit root relocation. The caller owns the transaction.
use crate::{Connection, StoreError};
use rusqlite::params;

pub fn asset_paths(connection: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare("SELECT asset_path AS path FROM asset_tags UNION SELECT asset_path AS path FROM asset_tag_sources UNION SELECT asset_path AS path FROM asset_tag_xmp_state UNION SELECT asset_path AS path FROM tag_xmp_sync_queue UNION SELECT asset_path AS path FROM person_manual_instances UNION SELECT reference_asset_path AS path FROM historical_people UNION SELECT asset_path AS path FROM person_tag_overrides UNION SELECT path AS path FROM indexed_assets UNION SELECT asset_path AS path FROM person_features_cache UNION SELECT asset_path AS path FROM person_instances_cache")?;
    Ok(statement
        .query_map([], |row| row.get("path"))?
        .collect::<Result<_, _>>()?)
}

pub fn folder_paths(connection: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare("SELECT folder_path AS path FROM folder_people UNION SELECT folder_path AS path FROM person_manual_instances UNION SELECT folder_path AS path FROM person_analysis_heads")?;
    Ok(statement
        .query_map([], |row| row.get("path"))?
        .collect::<Result<_, _>>()?)
}

pub fn identity_revisions(connection: &Connection, path: &str) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare("SELECT source_identity_revision AS revision FROM person_manual_instances WHERE asset_path=?1 AND source_identity_revision IS NOT NULL UNION SELECT source_revision AS revision FROM person_features_cache WHERE asset_path=?1 UNION SELECT source_revision AS revision FROM person_instances_cache WHERE asset_path=?1")?;
    Ok(statement
        .query_map([path], |row| row.get("revision"))?
        .collect::<Result<_, _>>()?)
}

pub fn replace_root(
    connection: &Connection,
    source: &str,
    destination: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE library_roots SET path=?2 WHERE path=?1",
        params![source, destination],
    )?;
    Ok(())
}

pub fn move_asset(
    connection: &Connection,
    source: &str,
    destination: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE asset_tags SET asset_path=?2 WHERE asset_path=?1",
        params![source, destination],
    )?;
    connection.execute(
        "UPDATE asset_tag_sources SET asset_path=?2 WHERE asset_path=?1",
        params![source, destination],
    )?;
    connection.execute(
        "UPDATE asset_tag_xmp_state SET asset_path=?2 WHERE asset_path=?1",
        params![source, destination],
    )?;
    connection.execute(
        "UPDATE tag_xmp_sync_queue SET asset_path=?2 WHERE asset_path=?1",
        params![source, destination],
    )?;
    connection.execute(
        "UPDATE person_manual_instances SET asset_path=?2 WHERE asset_path=?1",
        params![source, destination],
    )?;
    connection.execute(
        "UPDATE historical_people SET reference_asset_path=?2 WHERE reference_asset_path=?1",
        params![source, destination],
    )?;
    connection.execute(
        "UPDATE person_tag_overrides SET asset_path=?2 WHERE asset_path=?1",
        params![source, destination],
    )?;
    Ok(())
}

pub fn move_folder(
    connection: &Connection,
    source: &str,
    destination: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE folder_people SET folder_path=?2,revision=revision+1 WHERE folder_path=?1",
        params![source, destination],
    )?;
    connection.execute("UPDATE person_manual_instances SET folder_path=?2,revision=revision+1 WHERE folder_path=?1", params![source,destination])?;
    // Fence all old workers before they can publish to a relocated scope.
    connection.execute(
        "UPDATE person_analysis_heads SET generation=generation+1 WHERE folder_path=?1",
        [source],
    )?;
    connection.execute("UPDATE person_analysis_runs SET state='cancelled',updated_at=unixepoch() WHERE folder_path=?1 AND state IN ('queued','running')", [source])?;
    Ok(())
}

pub fn rebind_instances(
    connection: &Connection,
    path: &str,
    old_revision: Option<&str>,
    new_revision: Option<&str>,
    verified: bool,
) -> Result<(), StoreError> {
    connection.execute("UPDATE person_manual_instances SET needs_review=CASE WHEN ?4 AND source_identity_revision=?2 THEN needs_review ELSE 1 END, source_identity_revision=CASE WHEN ?4 AND source_identity_revision=?2 THEN ?3 ELSE source_identity_revision END,revision=revision+1 WHERE asset_path=?1", params![path,old_revision,new_revision,verified])?;
    Ok(())
}

pub fn rebind_features(
    connection: &Connection,
    source: &str,
    destination: &str,
    folder: &str,
    old_revision: &str,
    new_revision: &str,
) -> Result<usize, StoreError> {
    Ok(connection.execute("UPDATE person_features_cache SET asset_path=?2,folder_path=?3,source_revision=?5 WHERE asset_path=?1 AND source_revision=?4", params![source,destination,folder,old_revision,new_revision])?)
}
pub fn rebind_detections(
    connection: &Connection,
    source: &str,
    destination: &str,
    folder: &str,
    old_revision: &str,
    new_revision: &str,
) -> Result<(), StoreError> {
    connection.execute("UPDATE person_instances_cache SET asset_path=?2,folder_path=?3,source_revision=?5 WHERE asset_path=?1 AND source_revision=?4", params![source,destination,folder,old_revision,new_revision])?;
    Ok(())
}

pub fn subjects(connection: &Connection, folder: &str) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare("SELECT id FROM folder_people WHERE folder_path=?1")?;
    Ok(statement
        .query_map([folder], |row| row.get("id"))?
        .collect::<Result<_, _>>()?)
}
