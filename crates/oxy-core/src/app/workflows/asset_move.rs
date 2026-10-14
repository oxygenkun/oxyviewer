//! Atomic application row changes after a safe file move.
use crate::storage::{Connection, StoreError};
pub fn relocate_asset(
    connection: &Connection,
    source: &str,
    destination: &str,
    same_folder: bool,
) -> Result<(), StoreError> {
    crate::tags::repo::copy_source_rows(connection, source, destination, same_folder)?;
    crate::tags::repo::copy_effective_rows(connection, source, destination, same_folder)?;
    if same_folder {
        crate::people::repo::relocation::rename_cached_features(connection, source, destination)?;
        crate::people::repo::relocation::move_asset(connection, source, destination)?;
    } else {
        crate::people::repo::relocation::forget_cached_features(connection, source)?;
        crate::people::repo::relocation::require_review_for_asset(connection, source)?;
    }
    crate::tags::repo::move_xmp_state(connection, source, destination)?;
    crate::tags::repo::forget_asset_rows(connection, source)
}
