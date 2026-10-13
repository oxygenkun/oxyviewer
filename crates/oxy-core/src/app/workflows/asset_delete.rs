//! Application row changes after an explicit file deletion.
use crate::storage::{Connection, StoreError};
pub fn forget_asset(connection: &Connection, asset_path: &str) -> Result<(), StoreError> {
    crate::people::repo::manual::forget_cached_features(connection, asset_path)?;
    crate::tags::repo::forget_asset_rows(connection, asset_path)
}
