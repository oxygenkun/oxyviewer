//! The functions that read or write more than one domain at once.
//!
//! A person identity can push a tag onto an asset: the rule comes from the
//! person side (which identity is bound to which tag, and whether the user
//! rejected it for this photo) and the effect lands in the tag side (a `person`
//! source and the effective assignment derived from it). No domain crate can
//! own that without depending on a sibling, and the plan's whole point is that
//! the siblings do not depend on each other — so the statement lives here.
//!
//! The same is true of a file that moved or was deleted. One asset's path
//! changing is not one domain's business: its tag assignments and mirrored
//! keywords follow, its person instances and identity references follow, and
//! its cached features follow. [`relocate_asset`] and [`forget_asset`] are the
//! acts that span all three, which is exactly why they are not in a domain
//! crate.
//!
//! What stays out of here is the atomicity: these take the transaction a caller
//! opened, so a user action can still be one act.

use crate::{StoreError, repo};
use oxy_domain::CustomTagId;
use rusqlite::Connection;
use std::collections::HashMap;

/// The source kind a person identity writes when it tags an asset.
const PERSON_SOURCE: &str = "person";

/// Carries every row that names an asset to a new path, after the file moved.
///
/// A move inside one folder is still the same photo: the person facts and the
/// cached features follow the new name, so a review already made stays valid.
/// A move to another folder is a different asset — the person side is left
/// behind for review and the cache is dropped, because nothing has looked at
/// the new path yet. The tag vocabulary is unaffected either way: the file
/// carries the same tags wherever it lives.
pub fn relocate_asset(
    connection: &Connection,
    source: &str,
    destination: &str,
    same_folder: bool,
) -> Result<(), StoreError> {
    repo::tags::copy_source_rows(connection, source, destination, same_folder)?;
    repo::tags::copy_effective_rows(connection, source, destination, same_folder)?;
    if same_folder {
        repo::people::rename_cached_features(connection, source, destination)?;
        repo::people::repoint_asset_rows(connection, source, destination)?;
    } else {
        repo::people::forget_cached_features(connection, source)?;
        repo::people::require_review_for_asset(connection, source)?;
    }
    repo::tags::move_xmp_state(connection, source, destination)?;
    repo::tags::forget_asset_rows(connection, source)
}

/// Drops every row belonging to an asset that is gone.
///
/// Deleting a photo is not one domain's business either: its cached features
/// and its tag rows go in the same act, so nothing is left pointing at a path
/// that no longer exists.
pub fn forget_asset(connection: &Connection, asset_path: &str) -> Result<(), StoreError> {
    repo::people::forget_cached_features(connection, asset_path)?;
    repo::tags::forget_asset_rows(connection, asset_path)
}

/// Makes the tag assignments of one asset agree with what the person identity
/// currently says about it.
///
/// The identity's claim is a single tag: whatever it was before is dropped, the
/// current one is recorded, and every touched tag has its effective assignment
/// re-derived. The XMP mirror is owed a write only when the effective set of
/// this asset actually changed, so a no-op reconcile stays free.
pub fn reconcile_person_source_for_asset(
    connection: &Connection,
    subject_id: &str,
    asset_path: &str,
) -> Result<(), StoreError> {
    let desired = repo::people::person_source_tag(connection, subject_id, asset_path)?;
    let previous =
        repo::tags::source_tag_ids_of(connection, asset_path, PERSON_SOURCE, subject_id)?;
    let mut affected = previous;
    if let Some(tag_id) = desired {
        affected.insert(tag_id);
    }
    let before = affected
        .iter()
        .map(|tag_id| {
            repo::tags::effective_assignment_present(connection, asset_path, *tag_id)
                .map(|present| (*tag_id, present))
        })
        .collect::<Result<HashMap<CustomTagId, bool>, StoreError>>()?;

    repo::tags::delete_sources_of(connection, asset_path, PERSON_SOURCE, subject_id)?;
    if let Some(tag_id) = desired {
        repo::tags::insert_source(connection, asset_path, tag_id, PERSON_SOURCE, subject_id)?;
    }

    let mut changed = false;
    for tag_id in affected {
        repo::tags::reconcile_effective(connection, asset_path, tag_id)?;
        let after = repo::tags::effective_assignment_present(connection, asset_path, tag_id)?;
        changed |= before.get(&tag_id).copied().unwrap_or(false) != after;
    }
    if changed {
        repo::tags::enqueue_sync(connection, &[asset_path.to_owned()])
    } else {
        Ok(())
    }
}

/// Runs the reconcile for every asset one identity has a claim about.
///
/// An identity reaches an asset either because a review decision was recorded
/// for one of its instances, or because the asset already carries a `person`
/// source that now needs to be withdrawn.
pub fn reconcile_person_sources_for_subject(
    connection: &Connection,
    subject_id: &str,
) -> Result<(), StoreError> {
    for asset_path in person_source_asset_paths(connection, subject_id)? {
        reconcile_person_source_for_asset(connection, subject_id, &asset_path)?;
    }
    Ok(())
}

/// Every asset one identity could tag or is already tagging.
pub fn person_source_asset_paths(
    connection: &Connection,
    subject_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT i.asset_path FROM person_manual_instances i
         JOIN person_review_decisions r ON r.instance_id=i.id WHERE r.subject_id=?1
         UNION SELECT asset_path FROM asset_tag_sources
         WHERE source_kind='person' AND source_id=?1",
    )?;
    Ok(statement
        .query_map([subject_id], |row| row.get::<_, String>("asset_path"))?
        .collect::<Result<Vec<_>, _>>()?)
}
