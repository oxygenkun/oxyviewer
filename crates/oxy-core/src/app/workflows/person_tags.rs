//! Reconcile person claims without changing manual or sidecar sources.
use crate::storage::{Connection, StoreError};
use crate::tags::types::CustomTagId;
use std::collections::HashMap;

const PERSON_SOURCE: &str = "person";

pub fn reconcile_global_person_sources(
    connection: &Connection,
    person: &str,
) -> Result<(), StoreError> {
    let source = format!("global:{person}");
    let paths = crate::people::repo::global::review_asset_paths(connection, person)?
        .into_iter()
        .chain(crate::tags::repo::source_asset_paths(
            connection,
            PERSON_SOURCE,
            &source,
        )?)
        .collect::<std::collections::BTreeSet<_>>();
    for path in paths {
        let desired = crate::people::repo::global::person_source_tag(connection, person, &path)?;
        let mut affected =
            crate::tags::repo::source_tag_ids_of(connection, &path, PERSON_SOURCE, &source)?;
        if let Some(tag) = desired {
            affected.insert(tag);
        }
        let before = affected
            .iter()
            .map(|tag| {
                crate::tags::repo::effective_assignment_present(connection, &path, *tag)
                    .map(|value| (*tag, value))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        crate::tags::repo::delete_sources_of(connection, &path, PERSON_SOURCE, &source)?;
        if let Some(tag) = desired {
            crate::tags::repo::insert_source(connection, &path, tag, PERSON_SOURCE, &source)?;
        }
        let mut changed = false;
        for tag in affected {
            crate::tags::repo::reconcile_effective(connection, &path, tag)?;
            changed |= before.get(&tag).copied().unwrap_or(false)
                != crate::tags::repo::effective_assignment_present(connection, &path, tag)?;
        }
        if changed {
            crate::tags::repo::enqueue_sync(connection, &[path])?;
        }
    }
    Ok(())
}

pub fn reconcile_person_source_for_asset(
    connection: &Connection,
    subject_id: &str,
    asset_path: &str,
) -> Result<(), StoreError> {
    let desired =
        crate::people::repo::manual::person_source_tag(connection, subject_id, asset_path)?;
    let previous =
        crate::tags::repo::source_tag_ids_of(connection, asset_path, PERSON_SOURCE, subject_id)?;
    let mut affected = previous;
    if let Some(tag_id) = desired {
        affected.insert(tag_id);
    }
    let before = affected
        .iter()
        .map(|tag_id| {
            crate::tags::repo::effective_assignment_present(connection, asset_path, *tag_id)
                .map(|present| (*tag_id, present))
        })
        .collect::<Result<HashMap<CustomTagId, bool>, StoreError>>()?;

    crate::tags::repo::delete_sources_of(connection, asset_path, PERSON_SOURCE, subject_id)?;
    if let Some(tag_id) = desired {
        crate::tags::repo::insert_source(
            connection,
            asset_path,
            tag_id,
            PERSON_SOURCE,
            subject_id,
        )?;
    }

    let mut changed = false;
    for tag_id in affected {
        crate::tags::repo::reconcile_effective(connection, asset_path, tag_id)?;
        let after =
            crate::tags::repo::effective_assignment_present(connection, asset_path, tag_id)?;
        changed |= before.get(&tag_id).copied().unwrap_or(false) != after;
    }
    if changed {
        crate::tags::repo::enqueue_sync(connection, &[asset_path.to_owned()])
    } else {
        Ok(())
    }
}

pub fn reconcile_person_sources_for_subject(
    connection: &Connection,
    subject_id: &str,
) -> Result<(), StoreError> {
    for asset_path in person_source_asset_paths(connection, subject_id)? {
        reconcile_person_source_for_asset(connection, subject_id, &asset_path)?;
    }
    Ok(())
}

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
