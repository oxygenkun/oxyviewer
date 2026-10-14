//! Read projection of paths known by several features.
use crate::storage::{Connection, StoreError};
pub fn asset_paths(connection: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare("SELECT asset_path AS path FROM asset_tags UNION SELECT asset_path AS path FROM asset_tag_sources UNION SELECT asset_path AS path FROM asset_tag_xmp_state UNION SELECT asset_path AS path FROM tag_xmp_sync_queue UNION SELECT asset_path AS path FROM person_manual_instances UNION SELECT path AS path FROM indexed_assets UNION SELECT asset_path AS path FROM person_features_cache UNION SELECT asset_path AS path FROM person_instances_cache")?;
    Ok(statement
        .query_map([], |row| row.get("path"))?
        .collect::<Result<_, _>>()?)
}
