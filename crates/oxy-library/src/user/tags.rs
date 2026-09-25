//! The tag vocabulary the user built, the assignments they made, and the XMP
//! state that mirrors them.
//!
//! Tags are facts the user entered, so they are never part of a cache rebuild;
//! only an explicit delete removes them. The tables and their `user` class are
//! declared in `oxy_store::schema::user`, next to the DDL. What stays here is
//! what a tag *is*: that the name is unique among siblings, that a tag cannot
//! be moved below itself, and what deleting one cascades into.

use crate::{
    Library, LibraryError,
    cache::features::{forget_asset, rename_asset},
};
use oxy_domain::{
    AssetTagAssignment, AssetTagAssignmentsByPath, CustomTag, CustomTagId, PersonTagLink,
    PersonTagOverride, SetPersonTagLink, SetPersonTagOverride, TagDeleteImpact, TagSyncStatus,
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::{collections::HashMap, path::Path};

fn reconcile_effective_tag(
    transaction: &Transaction<'_>,
    path: &str,
    tag_id: CustomTagId,
) -> Result<(), rusqlite::Error> {
    let has_source: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM asset_tag_sources WHERE asset_path=?1 AND tag_id=?2) AS present",
        params![path, tag_id],
        |row| row.get("present"),
    )?;
    if has_source {
        transaction.execute(
            "INSERT OR IGNORE INTO asset_tags(asset_path,tag_id) VALUES (?1,?2)",
            params![path, tag_id],
        )?;
    } else {
        transaction.execute(
            "DELETE FROM asset_tags WHERE asset_path=?1 AND tag_id=?2",
            params![path, tag_id],
        )?;
    }
    Ok(())
}

pub(crate) fn reconcile_person_source_for_subject_asset(
    transaction: &Transaction<'_>,
    subject_id: &str,
    asset_path: &str,
) -> Result<(), rusqlite::Error> {
    let desired: Option<CustomTagId> = transaction
        .query_row(
            "SELECT t.tag_id FROM folder_historical_links l
         JOIN person_tag_links t ON t.historical_person_id=l.historical_person_id
         WHERE l.subject_id=?1 AND t.enabled=1 AND t.tag_id IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM person_tag_overrides o
             WHERE o.historical_person_id=l.historical_person_id
               AND o.asset_path=?2 AND o.suppressed=1)
           AND EXISTS (SELECT 1 FROM person_review_decisions r
             JOIN person_manual_instances i ON i.id=r.instance_id
             WHERE r.subject_id=?1 AND i.asset_path=?2
               AND r.decision='belongs' AND i.needs_review=0)",
            params![subject_id, asset_path],
            |row| row.get("tag_id"),
        )
        .optional()?;
    let old = {
        let mut statement = transaction.prepare(
            "SELECT tag_id FROM asset_tag_sources
             WHERE asset_path=?1 AND source_kind='person' AND source_id=?2",
        )?;
        statement
            .query_map(params![asset_path, subject_id], |row| {
                row.get::<_, CustomTagId>("tag_id")
            })?
            .collect::<Result<std::collections::HashSet<_>, _>>()?
    };
    let mut affected = old;
    if let Some(tag_id) = desired {
        affected.insert(tag_id);
    }
    let before = affected
        .iter()
        .map(|tag_id| {
            transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM asset_tags WHERE asset_path=?1 AND tag_id=?2) AS present",
                    params![asset_path, tag_id],
                    |row| row.get::<_, bool>("present"),
                )
                .map(|present| (*tag_id, present))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    transaction.execute(
        "DELETE FROM asset_tag_sources WHERE asset_path=?1 AND source_kind='person' AND source_id=?2",
        params![asset_path,subject_id],
    )?;
    if let Some(tag_id) = desired {
        transaction.execute(
            "INSERT INTO asset_tag_sources(asset_path,tag_id,source_kind,source_id)
             VALUES (?1,?2,'person',?3)",
            params![asset_path, tag_id, subject_id],
        )?;
    }
    let mut changed = false;
    for tag_id in affected {
        reconcile_effective_tag(transaction, asset_path, tag_id)?;
        let after: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM asset_tags WHERE asset_path=?1 AND tag_id=?2) AS present",
            params![asset_path, tag_id],
            |row| row.get("present"),
        )?;
        changed |= before.get(&tag_id).copied().unwrap_or(false) != after;
    }
    if changed {
        enqueue_paths(transaction, &[asset_path.to_owned()])?;
    }
    Ok(())
}

pub(crate) fn reconcile_person_sources_for_subject(
    transaction: &Transaction<'_>,
    subject_id: &str,
) -> Result<(), rusqlite::Error> {
    let paths = {
        let mut statement = transaction.prepare(
            "SELECT DISTINCT i.asset_path FROM person_manual_instances i
             JOIN person_review_decisions r ON r.instance_id=i.id WHERE r.subject_id=?1
             UNION SELECT asset_path FROM asset_tag_sources
             WHERE source_kind='person' AND source_id=?1",
        )?;
        statement
            .query_map([subject_id], |row| row.get::<_, String>("asset_path"))?
            .collect::<Result<Vec<_>, _>>()?
    };
    for path in paths {
        reconcile_person_source_for_subject_asset(transaction, subject_id, &path)?;
    }
    Ok(())
}

impl Library {
    pub fn asset_tag_source_kinds(
        &self,
        path: &Path,
        tag_id: CustomTagId,
    ) -> Result<Vec<String>, LibraryError> {
        let connection = self.read_connection();
        let mut statement = connection.prepare(
            "SELECT DISTINCT source_kind FROM asset_tag_sources WHERE asset_path=?1 AND tag_id=?2 ORDER BY source_kind",
        )?;
        Ok(statement
            .query_map(params![path.to_string_lossy(), tag_id], |row| {
                row.get("source_kind")
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_person_tag_link(
        &self,
        folder_path: &Path,
        subject_id: &str,
    ) -> Result<Option<PersonTagLink>, LibraryError> {
        let connection = self.read_connection();
        Ok(connection
            .query_row(
                "SELECT t.historical_person_id,t.tag_id,t.enabled,t.revision
             FROM folder_people f JOIN folder_historical_links l ON l.subject_id=f.id
             LEFT JOIN person_tag_links t ON t.historical_person_id=l.historical_person_id
             WHERE f.folder_path=?1 AND f.id=?2 AND t.historical_person_id IS NOT NULL",
                params![folder_path.to_string_lossy(), subject_id],
                |row| {
                    Ok(PersonTagLink {
                        historical_person_id: row.get("historical_person_id")?,
                        tag_id: row.get("tag_id")?,
                        enabled: row.get::<_, bool>("enabled")?
                            && row.get::<_, Option<i64>>("tag_id")?.is_some(),
                        revision: row.get("revision")?,
                    })
                },
            )
            .optional()?)
    }

    pub fn set_person_tag_link(
        &self,
        input: &SetPersonTagLink,
    ) -> Result<PersonTagLink, LibraryError> {
        if input.request_id.is_empty() || (input.enabled && input.tag_id.is_none()) {
            return Err(LibraryError::PersonConflict);
        }
        let mut connection = self.write();
        let tx = connection.transaction()?;
        let historical_id: String = tx
            .query_row(
                "SELECT l.historical_person_id FROM folder_people f
             JOIN folder_historical_links l ON l.subject_id=f.id
             WHERE f.folder_path=?1 AND f.id=?2",
                params![input.folder_path.to_string_lossy(), input.subject_id],
                |row| row.get("historical_person_id"),
            )
            .optional()?
            .ok_or(LibraryError::MissingPersonRecord)?;
        let replay: Option<(String, String)> = tx
            .query_row(
                "SELECT operation,entity_id FROM person_request_results WHERE request_id=?1",
                [&input.request_id],
                |row| Ok((row.get("operation")?, row.get("entity_id")?)),
            )
            .optional()?;
        if let Some((operation, id)) = replay {
            if operation != "setPersonTagLink" || id != historical_id {
                return Err(LibraryError::PersonConflict);
            }
        } else {
            let revision: Option<i64> = tx
                .query_row(
                    "SELECT revision FROM person_tag_links WHERE historical_person_id=?1",
                    [&historical_id],
                    |row| row.get("revision"),
                )
                .optional()?;
            if revision.unwrap_or(0) != input.expected_revision {
                return Err(LibraryError::PersonConflict);
            }
            if let Some(tag_id) = input.tag_id {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM custom_tags WHERE id=?1) AS present",
                    [tag_id],
                    |row| row.get("present"),
                )?;
                if !exists {
                    return Err(LibraryError::MissingTagParent);
                }
            }
            tx.execute(
                "INSERT INTO person_tag_links(historical_person_id,tag_id,enabled,revision)
                 VALUES (?1,?2,?3,1)
                 ON CONFLICT(historical_person_id) DO UPDATE SET
                   tag_id=excluded.tag_id,enabled=excluded.enabled,
                   revision=person_tag_links.revision+1,updated_at=unixepoch()",
                params![historical_id, input.tag_id, input.enabled],
            )?;
            let subjects = {
                let mut statement = tx.prepare(
                    "SELECT subject_id FROM folder_historical_links WHERE historical_person_id=?1",
                )?;
                statement
                    .query_map([&historical_id], |row| row.get::<_, String>("subject_id"))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            for subject in subjects {
                reconcile_person_sources_for_subject(&tx, &subject)?;
            }
            tx.execute(
                "INSERT INTO person_request_results VALUES (?1,'setPersonTagLink',?2)",
                params![input.request_id, historical_id],
            )?;
        }
        let result = tx.query_row(
            "SELECT historical_person_id,tag_id,enabled,revision FROM person_tag_links
             WHERE historical_person_id=?1",
            [&historical_id],
            |row| {
                Ok(PersonTagLink {
                    historical_person_id: row.get("historical_person_id")?,
                    tag_id: row.get("tag_id")?,
                    enabled: row.get::<_, bool>("enabled")?
                        && row.get::<_, Option<i64>>("tag_id")?.is_some(),
                    revision: row.get("revision")?,
                })
            },
        )?;
        tx.commit()?;
        Ok(result)
    }

    pub fn get_person_tag_override(
        &self,
        folder_path: &Path,
        subject_id: &str,
        asset_path: &Path,
    ) -> Result<Option<PersonTagOverride>, LibraryError> {
        let connection = self.read_connection();
        Ok(connection
            .query_row(
                "SELECT o.historical_person_id,o.asset_path,o.suppressed,o.revision
             FROM folder_people f JOIN folder_historical_links l ON l.subject_id=f.id
             JOIN person_tag_overrides o ON o.historical_person_id=l.historical_person_id
             WHERE f.folder_path=?1 AND f.id=?2 AND o.asset_path=?3",
                params![
                    folder_path.to_string_lossy(),
                    subject_id,
                    asset_path.to_string_lossy()
                ],
                |row| {
                    Ok(PersonTagOverride {
                        historical_person_id: row.get("historical_person_id")?,
                        asset_path: row.get::<_, String>("asset_path")?.into(),
                        suppressed: row.get("suppressed")?,
                        revision: row.get("revision")?,
                    })
                },
            )
            .optional()?)
    }

    pub fn set_person_tag_override(
        &self,
        input: &SetPersonTagOverride,
    ) -> Result<PersonTagOverride, LibraryError> {
        if input.request_id.is_empty()
            || input.asset_path.parent() != Some(input.folder_path.as_path())
        {
            return Err(LibraryError::PersonConflict);
        }
        let mut connection = self.write();
        let tx = connection.transaction()?;
        let historical_id: String = tx
            .query_row(
                "SELECT l.historical_person_id FROM folder_people f
             JOIN folder_historical_links l ON l.subject_id=f.id
             WHERE f.folder_path=?1 AND f.id=?2",
                params![input.folder_path.to_string_lossy(), input.subject_id],
                |row| row.get("historical_person_id"),
            )
            .optional()?
            .ok_or(LibraryError::MissingPersonRecord)?;
        let request_entity = format!("{}:{}", historical_id, input.asset_path.display());
        let replay: Option<(String, String)> = tx
            .query_row(
                "SELECT operation,entity_id FROM person_request_results WHERE request_id=?1",
                [&input.request_id],
                |row| Ok((row.get("operation")?, row.get("entity_id")?)),
            )
            .optional()?;
        if let Some((operation, entity)) = replay {
            if operation != "setPersonTagOverride" || entity != request_entity {
                return Err(LibraryError::PersonConflict);
            }
        } else {
            let revision: Option<i64> = tx.query_row(
                "SELECT revision FROM person_tag_overrides WHERE historical_person_id=?1 AND asset_path=?2",
                params![historical_id,input.asset_path.to_string_lossy()], |row| row.get("revision"),
            ).optional()?;
            if revision.unwrap_or(0) != input.expected_revision {
                return Err(LibraryError::PersonConflict);
            }
            tx.execute(
                "INSERT INTO person_tag_overrides(historical_person_id,asset_path,suppressed,revision)
                 VALUES (?1,?2,?3,1)
                 ON CONFLICT(historical_person_id,asset_path) DO UPDATE SET
                   suppressed=excluded.suppressed,revision=person_tag_overrides.revision+1",
                params![historical_id,input.asset_path.to_string_lossy(),input.suppressed],
            )?;
            let subjects = {
                let mut statement = tx.prepare(
                    "SELECT subject_id FROM folder_historical_links WHERE historical_person_id=?1",
                )?;
                statement
                    .query_map([&historical_id], |row| row.get::<_, String>("subject_id"))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            for subject in subjects {
                reconcile_person_source_for_subject_asset(
                    &tx,
                    &subject,
                    &input.asset_path.to_string_lossy(),
                )?;
            }
            tx.execute(
                "INSERT INTO person_request_results VALUES (?1,'setPersonTagOverride',?2)",
                params![input.request_id, request_entity],
            )?;
        }
        let result = tx.query_row(
            "SELECT historical_person_id,asset_path,suppressed,revision FROM person_tag_overrides
             WHERE historical_person_id=?1 AND asset_path=?2",
            params![historical_id, input.asset_path.to_string_lossy()],
            |row| {
                Ok(PersonTagOverride {
                    historical_person_id: row.get("historical_person_id")?,
                    asset_path: row.get::<_, String>("asset_path")?.into(),
                    suppressed: row.get("suppressed")?,
                    revision: row.get("revision")?,
                })
            },
        )?;
        tx.commit()?;
        Ok(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagXmpPayload {
    pub subjects: Vec<String>,
    pub hierarchical: Vec<String>,
    pub previous_subjects: Vec<String>,
    pub previous_hierarchical: Vec<String>,
}

fn normalize_name(name: &str) -> Result<(String, String), LibraryError> {
    let name = name.trim();
    if name.is_empty() || name.contains('|') || name.chars().any(char::is_control) {
        return Err(LibraryError::InvalidTagName);
    }
    Ok((name.to_owned(), name.to_lowercase()))
}

fn map_constraint(error: rusqlite::Error) -> LibraryError {
    if matches!(
        error,
        rusqlite::Error::SqliteFailure(ref value, _)
            if value.code == rusqlite::ErrorCode::ConstraintViolation
    ) {
        LibraryError::DuplicateTagName
    } else {
        LibraryError::Sqlite(error)
    }
}

fn tag_exists(transaction: &Transaction<'_>, id: CustomTagId) -> Result<bool, rusqlite::Error> {
    transaction
        .query_row(
            "SELECT 1 FROM custom_tags WHERE id = ?1",
            params![id],
            |_| Ok(true),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
}

fn enqueue_paths(transaction: &Transaction<'_>, paths: &[String]) -> Result<(), rusqlite::Error> {
    for path in paths {
        transaction.execute(
            "INSERT INTO tag_xmp_sync_queue(asset_path, requested_at, attempt_count, last_error)
             VALUES (?1, unixepoch(), 0, NULL)
             ON CONFLICT(asset_path) DO UPDATE SET
               requested_at=excluded.requested_at, attempt_count=0, last_error=NULL",
            params![path],
        )?;
    }
    Ok(())
}

impl Library {
    /// Match explicit assignments against selected subtrees in one SQLite read.
    /// Only the supplied directory snapshot participates; no index or metadata is needed.
    pub fn filter_assets_by_tags(
        &self,
        assets: &[oxy_domain::AssetSummary],
        query: &oxy_domain::AssetQuery,
    ) -> Result<Vec<oxy_domain::AssetSummary>, LibraryError> {
        if query.tag_ids.is_empty() {
            return Ok(assets.to_vec());
        }
        let selected = serde_json::to_string(&query.tag_ids)?;
        let paths =
            serde_json::to_string(&assets.iter().map(|asset| &asset.path).collect::<Vec<_>>())?;
        let connection = self.read_connection();
        let mut statement = connection.prepare(
            "WITH RECURSIVE selected(id) AS (
                SELECT DISTINCT value FROM json_each(?1)
             ), subtree(root, id) AS (
                SELECT id, id FROM selected
                UNION
                SELECT subtree.root, tag.id FROM custom_tags tag
                JOIN subtree ON tag.parent_id = subtree.id
             )
             SELECT assignment.asset_path FROM asset_tags assignment
             JOIN subtree ON assignment.tag_id = subtree.id
             WHERE assignment.asset_path IN (SELECT value FROM json_each(?2))
             GROUP BY assignment.asset_path
             HAVING ?3 OR COUNT(DISTINCT subtree.root) = (SELECT COUNT(*) FROM selected)",
        )?;
        let matches = statement
            .query_map(
                params![
                    selected,
                    paths,
                    query.tag_match == oxy_domain::TagMatchMode::Any
                ],
                |row| row.get::<_, String>("asset_path"),
            )?
            .collect::<Result<std::collections::HashSet<_>, _>>()?;
        Ok(assets
            .iter()
            .filter(|asset| matches.contains(asset.path.to_string_lossy().as_ref()))
            .cloned()
            .collect())
    }

    pub fn custom_tags(&self) -> Result<Vec<CustomTag>, LibraryError> {
        let connection = self.read_connection();
        let mut statement = connection.prepare(
            "SELECT id, parent_id, name, sort_order
             FROM custom_tags ORDER BY parent_id, sort_order, name COLLATE NOCASE, id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, CustomTagId>("id")?,
                    row.get::<_, Option<CustomTagId>>("parent_id")?,
                    row.get::<_, String>("name")?,
                    row.get::<_, i64>("sort_order")?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let names = rows
            .iter()
            .map(|(id, parent_id, name, _)| (*id, (*parent_id, name.clone())))
            .collect::<HashMap<_, _>>();
        Ok(rows
            .into_iter()
            .map(|(id, parent_id, name, sort_order)| CustomTag {
                id,
                parent_id,
                path: tag_path(id, &names),
                name,
                sort_order,
            })
            .collect())
    }

    pub fn create_custom_tag(
        &self,
        parent_id: Option<CustomTagId>,
        name: &str,
    ) -> Result<CustomTag, LibraryError> {
        let (name, name_key) = normalize_name(name)?;
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        if let Some(parent_id) = parent_id
            && !tag_exists(&transaction, parent_id)?
        {
            return Err(LibraryError::MissingTagParent);
        }
        let sort_order = transaction.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 AS sort_order FROM custom_tags
             WHERE parent_id IS ?1",
            params![parent_id],
            |row| row.get::<_, i64>("sort_order"),
        )?;
        transaction
            .execute(
                "INSERT INTO custom_tags(parent_id, name, name_key, sort_order)
                 VALUES (?1, ?2, ?3, ?4)",
                params![parent_id, name, name_key, sort_order],
            )
            .map_err(map_constraint)?;
        let id = transaction.last_insert_rowid();
        transaction.commit()?;
        drop(connection);
        self.custom_tags()?
            .into_iter()
            .find(|tag| tag.id == id)
            .ok_or(LibraryError::MissingTagParent)
    }

    pub fn update_custom_tag(
        &self,
        id: CustomTagId,
        parent_id: Option<CustomTagId>,
        name: &str,
    ) -> Result<CustomTag, LibraryError> {
        let (name, name_key) = normalize_name(name)?;
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let (current_parent_id, current_sort_order) = transaction
            .query_row(
                "SELECT parent_id, sort_order FROM custom_tags WHERE id=?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, Option<CustomTagId>>("parent_id")?,
                        row.get::<_, i64>("sort_order")?,
                    ))
                },
            )
            .optional()?
            .ok_or(LibraryError::MissingTagParent)?;
        if let Some(parent_id) = parent_id {
            if !tag_exists(&transaction, parent_id)? {
                return Err(LibraryError::MissingTagParent);
            }
            let creates_cycle = transaction.query_row(
                "WITH RECURSIVE descendants(id) AS (
                   SELECT id FROM custom_tags WHERE id = ?1
                   UNION ALL SELECT child.id FROM custom_tags child
                     JOIN descendants parent ON child.parent_id = parent.id
                 ) SELECT EXISTS(SELECT 1 FROM descendants WHERE id = ?2) AS present",
                params![id, parent_id],
                |row| row.get::<_, bool>("present"),
            )?;
            if creates_cycle {
                return Err(LibraryError::TagHierarchyCycle);
            }
        }
        let affected = descendant_asset_paths(&transaction, id)?;
        let sort_order = if current_parent_id == parent_id {
            current_sort_order
        } else {
            transaction.query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 AS sort_order FROM custom_tags
                 WHERE parent_id IS ?1 AND id != ?2",
                params![parent_id, id],
                |row| row.get::<_, i64>("sort_order"),
            )?
        };
        transaction
            .execute(
                "UPDATE custom_tags SET parent_id=?1, name=?2, name_key=?3,
                   sort_order=?4, updated_at=unixepoch() WHERE id=?5",
                params![parent_id, name, name_key, sort_order, id],
            )
            .map_err(map_constraint)?;
        enqueue_paths(&transaction, &affected)?;
        transaction.commit()?;
        drop(connection);
        self.custom_tags()?
            .into_iter()
            .find(|tag| tag.id == id)
            .ok_or(LibraryError::MissingTagParent)
    }

    pub fn custom_tag_delete_impact(
        &self,
        id: CustomTagId,
    ) -> Result<TagDeleteImpact, LibraryError> {
        let connection = self.read_connection();
        let (tag_count, asset_count) = connection.query_row(
            "WITH RECURSIVE descendants(id) AS (
               SELECT id FROM custom_tags WHERE id = ?1
               UNION ALL SELECT child.id FROM custom_tags child
                 JOIN descendants parent ON child.parent_id = parent.id
             ) SELECT COUNT(DISTINCT descendants.id) AS tag_count,
               COUNT(DISTINCT asset_tags.asset_path) AS asset_count
               FROM descendants LEFT JOIN asset_tags ON asset_tags.tag_id = descendants.id",
            params![id],
            |row| {
                Ok((
                    row.get::<_, usize>("tag_count")?,
                    row.get::<_, usize>("asset_count")?,
                ))
            },
        )?;
        Ok(TagDeleteImpact {
            tag_count,
            asset_count,
        })
    }

    pub fn delete_custom_tag(&self, id: CustomTagId) -> Result<TagDeleteImpact, LibraryError> {
        let impact = self.custom_tag_delete_impact(id)?;
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let affected = descendant_asset_paths(&transaction, id)?;
        transaction.execute("DELETE FROM custom_tags WHERE id = ?1", params![id])?;
        enqueue_paths(&transaction, &affected)?;
        transaction.commit()?;
        Ok(impact)
    }

    pub fn asset_tag_assignments(
        &self,
        paths: &[std::path::PathBuf],
    ) -> Result<Vec<AssetTagAssignment>, LibraryError> {
        let tags = self.custom_tags()?;
        let connection = self.read_connection();
        let mut assigned = HashMap::<CustomTagId, usize>::new();
        for path in paths {
            let mut statement =
                connection.prepare("SELECT tag_id FROM asset_tags WHERE asset_path = ?1")?;
            for id in
                statement.query_map(params![path.to_string_lossy()], |row| row.get("tag_id"))?
            {
                *assigned.entry(id?).or_default() += 1;
            }
        }
        Ok(tags
            .into_iter()
            .map(|tag| AssetTagAssignment {
                assigned_count: assigned.get(&tag.id).copied().unwrap_or_default(),
                asset_count: paths.len(),
                tag,
            })
            .collect())
    }

    /// Read the tag tree once and batch visible paths without changing the
    /// aggregate assignment contract used by the tag editor.
    pub fn asset_tag_assignments_by_path(
        &self,
        paths: &[std::path::PathBuf],
    ) -> Result<Vec<AssetTagAssignmentsByPath>, LibraryError> {
        let tags = self.custom_tags()?;
        let connection = self.read_connection();
        let mut assigned = HashMap::<String, std::collections::HashSet<CustomTagId>>::new();
        for chunk in paths.chunks(512) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let mut statement = connection.prepare(&format!(
                "SELECT asset_path, tag_id FROM asset_tags WHERE asset_path IN ({placeholders})"
            ))?;
            let rows = statement.query_map(
                rusqlite::params_from_iter(chunk.iter().map(|path| path.to_string_lossy())),
                |row| {
                    Ok((
                        row.get::<_, String>("asset_path")?,
                        row.get::<_, CustomTagId>("tag_id")?,
                    ))
                },
            )?;
            for row in rows {
                let (path, id) = row?;
                assigned.entry(path).or_default().insert(id);
            }
        }
        Ok(paths
            .iter()
            .map(|path| {
                let ids = assigned.get(path.to_string_lossy().as_ref());
                AssetTagAssignmentsByPath {
                    path: path.clone(),
                    assignments: tags
                        .iter()
                        .filter(|tag| ids.is_some_and(|ids| ids.contains(&tag.id)))
                        .cloned()
                        .map(|tag| AssetTagAssignment {
                            tag,
                            assigned_count: 1,
                            asset_count: 1,
                        })
                        .collect(),
                }
            })
            .collect())
    }

    pub fn set_asset_tag(
        &self,
        paths: &[std::path::PathBuf],
        tag_id: CustomTagId,
        assigned: bool,
    ) -> Result<(), LibraryError> {
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        if !tag_exists(&transaction, tag_id)? {
            return Err(LibraryError::MissingTagParent);
        }
        let path_strings = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for path in &path_strings {
            if assigned {
                transaction.execute(
                    "DELETE FROM asset_tag_sources WHERE asset_path=?1 AND tag_id=?2 AND source_kind='legacy'",
                    params![path, tag_id],
                )?;
                transaction.execute(
                    "INSERT OR IGNORE INTO asset_tag_sources(asset_path,tag_id,source_kind,source_id)
                     VALUES (?1,?2,'manual','')",
                    params![path,tag_id],
                )?;
            } else {
                transaction.execute(
                    "DELETE FROM asset_tag_sources WHERE asset_path=?1 AND tag_id=?2
                     AND source_kind IN ('manual','legacy')",
                    params![path, tag_id],
                )?;
            }
            reconcile_effective_tag(&transaction, path, tag_id)?;
        }
        enqueue_paths(&transaction, &path_strings)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_tag_sync_paths(&self) -> Result<Vec<std::path::PathBuf>, LibraryError> {
        let connection = self.read_connection();
        let mut statement = connection.prepare(
            "SELECT asset_path FROM tag_xmp_sync_queue ORDER BY requested_at, asset_path",
        )?;
        Ok(statement
            .query_map([], |row| row.get::<_, String>("asset_path"))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub fn tag_xmp_payload(&self, path: &Path) -> Result<TagXmpPayload, LibraryError> {
        let connection = self.read_connection();
        let tags = assigned_tag_paths(&connection, path)?;
        let subjects = tags
            .iter()
            .filter_map(|value| value.rsplit('|').next().map(str::to_owned))
            .collect::<Vec<_>>();
        let previous = connection
            .query_row(
                "SELECT subjects_json, hierarchical_json FROM asset_tag_xmp_state
                 WHERE asset_path = ?1",
                params![path.to_string_lossy()],
                |row| {
                    Ok((
                        row.get::<_, String>("subjects_json")?,
                        row.get::<_, String>("hierarchical_json")?,
                    ))
                },
            )
            .optional()?;
        let (previous_subjects, previous_hierarchical) = previous.map_or_else(
            || Ok((Vec::new(), Vec::new())),
            |(subjects, hierarchical)| {
                Ok::<_, serde_json::Error>((
                    serde_json::from_str(&subjects)?,
                    serde_json::from_str(&hierarchical)?,
                ))
            },
        )?;
        Ok(TagXmpPayload {
            subjects,
            hierarchical: tags,
            previous_subjects,
            previous_hierarchical,
        })
    }

    pub fn complete_tag_xmp_sync(
        &self,
        path: &Path,
        subjects: &[String],
        hierarchical: &[String],
    ) -> Result<(), LibraryError> {
        let connection = self.write();
        connection.execute(
            "INSERT INTO asset_tag_xmp_state(asset_path, subjects_json, hierarchical_json, synced_at)
             VALUES (?1, ?2, ?3, unixepoch())
             ON CONFLICT(asset_path) DO UPDATE SET subjects_json=excluded.subjects_json,
               hierarchical_json=excluded.hierarchical_json, synced_at=excluded.synced_at",
            params![
                path.to_string_lossy(),
                serde_json::to_string(subjects)?,
                serde_json::to_string(hierarchical)?,
            ],
        )?;
        let current_hierarchical = assigned_tag_paths(&connection, path)?;
        let current_subjects = current_hierarchical
            .iter()
            .filter_map(|value| value.rsplit('|').next().map(str::to_owned))
            .collect::<Vec<_>>();
        if subjects == current_subjects && hierarchical == current_hierarchical {
            connection.execute(
                "DELETE FROM tag_xmp_sync_queue WHERE asset_path = ?1",
                params![path.to_string_lossy()],
            )?;
        }
        Ok(())
    }

    pub fn fail_tag_xmp_sync(&self, path: &Path, error: &str) -> Result<(), LibraryError> {
        self.write().execute(
            "UPDATE tag_xmp_sync_queue SET attempt_count=attempt_count + 1, last_error=?2
             WHERE asset_path=?1",
            params![path.to_string_lossy(), error],
        )?;
        Ok(())
    }

    pub fn tag_sync_status(&self) -> Result<TagSyncStatus, LibraryError> {
        let connection = self.read_connection();
        let (pending_count, failed_count) = connection.query_row(
            "SELECT COUNT(*) AS pending_count, COUNT(last_error) AS failed_count FROM tag_xmp_sync_queue",
            [],
            |row| Ok((row.get("pending_count")?, row.get("failed_count")?)),
        )?;
        let last_error = connection
            .query_row(
                "SELECT last_error FROM tag_xmp_sync_queue WHERE last_error IS NOT NULL
                 ORDER BY requested_at DESC LIMIT 1",
                [],
                |row| row.get("last_error"),
            )
            .optional()?;
        Ok(TagSyncStatus {
            pending_count,
            failed_count,
            last_error,
        })
    }

    pub fn import_sidecar_tags(
        &self,
        path: &Path,
        subjects: &[String],
        hierarchical: &[String],
    ) -> Result<(), LibraryError> {
        let has_pending_write = self.write().query_row(
            "SELECT EXISTS(SELECT 1 FROM tag_xmp_sync_queue WHERE asset_path = ?1) AS present",
            params![path.to_string_lossy()],
            |row| row.get::<_, bool>("present"),
        )?;
        if has_pending_write {
            return Ok(());
        }

        let mut desired_tag_ids = std::collections::HashSet::new();
        for value in hierarchical {
            let mut parent_id = None;
            for segment in value.split('|') {
                let (name, name_key) = normalize_name(segment)?;
                let existing = self
                    .write()
                    .query_row(
                        "SELECT id FROM custom_tags WHERE parent_id IS ?1 AND name_key = ?2",
                        params![parent_id, name_key],
                        |row| row.get::<_, CustomTagId>("id"),
                    )
                    .optional()?;
                parent_id = Some(match existing {
                    Some(id) => id,
                    None => self.create_custom_tag(parent_id, &name)?.id,
                });
            }
            if let Some(tag_id) = parent_id {
                desired_tag_ids.insert(tag_id);
            }
        }
        let hierarchical_subjects = hierarchical
            .iter()
            .filter_map(|value| value.rsplit('|').next())
            .map(str::to_lowercase)
            .collect::<std::collections::HashSet<_>>();
        for subject in subjects {
            if hierarchical_subjects.contains(&subject.to_lowercase()) {
                continue;
            }
            let (name, name_key) = normalize_name(subject)?;
            let existing = self
                .write()
                .query_row(
                    "SELECT id FROM custom_tags WHERE parent_id IS NULL AND name_key = ?1",
                    params![name_key],
                    |row| row.get::<_, CustomTagId>("id"),
                )
                .optional()?;
            let tag_id = match existing {
                Some(id) => id,
                None => self.create_custom_tag(None, &name)?.id,
            };
            desired_tag_ids.insert(tag_id);
        }
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let existing_tag_ids = {
            let mut statement = transaction.prepare(
                "SELECT tag_id FROM asset_tag_sources
                 WHERE asset_path=?1 AND source_kind='sidecar'",
            )?;
            statement
                .query_map(params![path.to_string_lossy()], |row| {
                    row.get::<_, CustomTagId>("tag_id")
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for tag_id in existing_tag_ids {
            if !desired_tag_ids.contains(&tag_id) {
                transaction.execute(
                    "DELETE FROM asset_tag_sources WHERE asset_path=?1 AND tag_id=?2
                     AND source_kind='sidecar'",
                    params![path.to_string_lossy(), tag_id],
                )?;
                reconcile_effective_tag(&transaction, path.to_string_lossy().as_ref(), tag_id)?;
            }
        }
        for tag_id in &desired_tag_ids {
            transaction.execute(
                "INSERT OR IGNORE INTO asset_tag_sources(asset_path,tag_id,source_kind,source_id)
                 VALUES (?1,?2,'sidecar','')",
                params![path.to_string_lossy(), tag_id],
            )?;
            reconcile_effective_tag(&transaction, path.to_string_lossy().as_ref(), *tag_id)?;
        }
        let effective_tag_ids = {
            let mut statement =
                transaction.prepare("SELECT tag_id FROM asset_tags WHERE asset_path=?1")?;
            statement
                .query_map(params![path.to_string_lossy()], |row| {
                    row.get::<_, CustomTagId>("tag_id")
                })?
                .collect::<Result<std::collections::HashSet<_>, _>>()?
        };
        if effective_tag_ids != desired_tag_ids {
            enqueue_paths(&transaction, &[path.to_string_lossy().into_owned()])?;
        }
        transaction.commit()?;
        drop(connection);
        self.complete_tag_xmp_sync(path, subjects, hierarchical)
    }

    pub fn move_asset_tag_state(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), LibraryError> {
        self.transfer_asset_tag_state(source, destination, false)
    }

    pub fn copy_asset_tag_state(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), LibraryError> {
        self.transfer_asset_tag_state(source, destination, true)
    }

    fn transfer_asset_tag_state(
        &self,
        source: &Path,
        destination: &Path,
        copy: bool,
    ) -> Result<(), LibraryError> {
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let same_folder_move = !copy && source.parent() == destination.parent();
        transaction.execute(
            "INSERT OR IGNORE INTO asset_tag_sources(asset_path,tag_id,source_kind,source_id)
             SELECT ?2,tag_id,source_kind,source_id FROM asset_tag_sources
             WHERE asset_path=?1 AND (?3=1 OR source_kind!='person')",
            params![
                source.to_string_lossy(),
                destination.to_string_lossy(),
                same_folder_move
            ],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO asset_tags(asset_path, tag_id)
             SELECT DISTINCT ?2,tag_id FROM asset_tag_sources
             WHERE asset_path=?1 AND (?3=1 OR source_kind!='person')",
            params![
                source.to_string_lossy(),
                destination.to_string_lossy(),
                same_folder_move
            ],
        )?;
        if copy {
            enqueue_paths(&transaction, &[destination.to_string_lossy().into_owned()])?;
        } else {
            if same_folder_move {
                // The feature cache still describes the same photo, so point it
                // at the new path instead of throwing the vectors away.
                rename_asset(
                    &transaction,
                    &source.to_string_lossy(),
                    &destination.to_string_lossy(),
                )?;
                transaction.execute(
                    "UPDATE person_manual_instances SET asset_path=?2 WHERE asset_path=?1",
                    params![source.to_string_lossy(), destination.to_string_lossy()],
                )?;
                transaction.execute(
                    "UPDATE historical_people SET reference_asset_path=?2 WHERE reference_asset_path=?1",
                    params![source.to_string_lossy(),destination.to_string_lossy()],
                )?;
                transaction.execute(
                    "UPDATE person_tag_overrides SET asset_path=?2 WHERE asset_path=?1",
                    params![source.to_string_lossy(), destination.to_string_lossy()],
                )?;
            } else {
                forget_asset(&transaction, &source.to_string_lossy())?;
                transaction.execute(
                    "UPDATE person_manual_instances SET needs_review=1 WHERE asset_path=?1",
                    params![source.to_string_lossy()],
                )?;
            }
            transaction.execute(
                "UPDATE OR REPLACE asset_tag_xmp_state SET asset_path=?2 WHERE asset_path=?1",
                params![source.to_string_lossy(), destination.to_string_lossy()],
            )?;
            transaction.execute(
                "DELETE FROM asset_tag_sources WHERE asset_path=?1",
                params![source.to_string_lossy()],
            )?;
            transaction.execute(
                "DELETE FROM asset_tags WHERE asset_path=?1",
                params![source.to_string_lossy()],
            )?;
            transaction.execute(
                "DELETE FROM tag_xmp_sync_queue WHERE asset_path=?1",
                params![source.to_string_lossy()],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn remove_asset_tag_state(&self, path: &Path) -> Result<(), LibraryError> {
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let path = path.to_string_lossy();
        forget_asset(&transaction, &path)?;
        transaction.execute(
            "DELETE FROM asset_tag_sources WHERE asset_path=?1",
            params![path],
        )?;
        transaction.execute("DELETE FROM asset_tags WHERE asset_path=?1", params![path])?;
        transaction.execute(
            "DELETE FROM asset_tag_xmp_state WHERE asset_path=?1",
            params![path],
        )?;
        transaction.execute(
            "DELETE FROM tag_xmp_sync_queue WHERE asset_path=?1",
            params![path],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn tag_path(
    id: CustomTagId,
    names: &HashMap<CustomTagId, (Option<CustomTagId>, String)>,
) -> String {
    let mut parts = Vec::new();
    let mut current = Some(id);
    while let Some(id) = current {
        let Some((parent, name)) = names.get(&id) else {
            break;
        };
        parts.push(name.as_str());
        current = *parent;
    }
    parts.reverse();
    parts.join("|")
}

fn descendant_asset_paths(
    transaction: &Transaction<'_>,
    id: CustomTagId,
) -> Result<Vec<String>, rusqlite::Error> {
    let mut statement = transaction.prepare(
        "WITH RECURSIVE descendants(id) AS (
           SELECT id FROM custom_tags WHERE id = ?1
           UNION ALL SELECT child.id FROM custom_tags child
             JOIN descendants parent ON child.parent_id = parent.id
         ) SELECT DISTINCT asset_path FROM asset_tags
           WHERE tag_id IN (SELECT id FROM descendants)",
    )?;
    statement
        .query_map(params![id], |row| row.get("asset_path"))?
        .collect::<Result<Vec<_>, _>>()
}

fn assigned_tag_paths(
    connection: &rusqlite::Connection,
    path: &Path,
) -> Result<Vec<String>, LibraryError> {
    let tags = {
        let mut statement = connection.prepare(
            "SELECT tag.id, tag.parent_id, tag.name FROM custom_tags tag
             JOIN asset_tags assignment ON assignment.tag_id=tag.id
             WHERE assignment.asset_path=?1 ORDER BY tag.id",
        )?;
        statement
            .query_map(params![path.to_string_lossy()], |row| {
                Ok((
                    row.get::<_, CustomTagId>("id")?,
                    row.get::<_, Option<CustomTagId>>("parent_id")?,
                    row.get::<_, String>("name")?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let all_names = {
        let mut statement = connection.prepare("SELECT id, parent_id, name FROM custom_tags")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, CustomTagId>("id")?,
                    (
                        row.get::<_, Option<CustomTagId>>("parent_id")?,
                        row.get::<_, String>("name")?,
                    ),
                ))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?
    };
    Ok(tags
        .into_iter()
        .map(|(id, _, _)| tag_path(id, &all_names))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn filters_snapshot_by_subtrees_and_groups_before_paging() {
        use oxy_domain::{AssetQuery, AssetSummary, TagMatchMode};
        let library = Library::in_memory().unwrap();
        let parent = library.create_custom_tag(None, "People").unwrap();
        let child = library
            .create_custom_tag(Some(parent.id), "Family")
            .unwrap();
        let other = library.create_custom_tag(None, "Trips").unwrap();
        let same_name = library.create_custom_tag(Some(other.id), "Family").unwrap();
        let paths = [
            PathBuf::from("/photos/a.jpg"),
            PathBuf::from("/photos/b.jpg"),
            PathBuf::from("/elsewhere/c.jpg"),
        ];
        library.set_asset_tag(&paths[..1], child.id, true).unwrap();
        library.set_asset_tag(&paths, other.id, true).unwrap();
        library
            .set_asset_tag(&paths[1..2], same_name.id, true)
            .unwrap();
        let assets = paths[..2]
            .iter()
            .enumerate()
            .map(|(index, path)| AssetSummary {
                id: index.to_string(),
                path: path.clone(),
                name: format!("{index}.jpg"),
                extension: "jpg".into(),
                kind: oxy_domain::AssetKind::Jpeg,
                size_bytes: 1,
                modified_at_ms: 0,
                has_sidecar: false,
                rating: Some(4),
                color_label: Some("Red".into()),
                pick_label: Some(oxy_domain::PickLabel::Accepted),
            })
            .collect::<Vec<_>>();
        let mut query = AssetQuery {
            tag_ids: vec![parent.id],
            ..Default::default()
        };
        assert!(!query.needs_metadata_enrichment());
        assert_eq!(
            library
                .filter_assets_by_tags(&assets, &query)
                .unwrap()
                .len(),
            1
        );
        query.tag_ids.push(other.id);
        assert_eq!(
            library
                .filter_assets_by_tags(&assets, &query)
                .unwrap()
                .len(),
            1
        );
        query.tag_match = TagMatchMode::Any;
        query.minimum_rating = Some(4);
        query.color_labels = vec!["Red".into()];
        query.pick_labels = vec!["accepted".into()];
        query.page_size = Some(1);
        let matched = library.filter_assets_by_tags(&assets, &query).unwrap();
        let page = oxy_fs::page_assets(&matched, &query, 0);
        assert_eq!(page.total, 2);
        assert_eq!(page.items.len(), 1);
        assert_eq!(oxy_fs::page_assets(&matched, &query, 1).items.len(), 1);
        query.tag_ids = vec![child.id];
        library
            .update_custom_tag(child.id, Some(other.id), "Renamed")
            .unwrap();
        assert_eq!(
            library
                .filter_assets_by_tags(&assets, &query)
                .unwrap()
                .len(),
            1
        );
        query.tag_ids = vec![parent.id];
        assert!(
            library
                .filter_assets_by_tags(&assets, &query)
                .unwrap()
                .is_empty()
        );
        query.tag_ids = vec![child.id];
        library.delete_custom_tag(child.id).unwrap();
        assert!(
            library
                .filter_assets_by_tags(&assets, &query)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn manages_hierarchy_assignments_and_subtree_deletion() {
        let library = Library::in_memory().unwrap();
        let people = library.create_custom_tag(None, "People").unwrap();
        let family = library
            .create_custom_tag(Some(people.id), "Family")
            .unwrap();
        assert_eq!(people.name, "People");
        assert_eq!(family.name, "Family");
        assert_eq!(family.path, "People|Family");
        assert!(matches!(
            library.create_custom_tag(None, "people"),
            Err(LibraryError::DuplicateTagName)
        ));
        assert!(matches!(
            library.create_custom_tag(Some(people.id), "family"),
            Err(LibraryError::DuplicateTagName)
        ));
        let asset = PathBuf::from("/photos/a.jpg");
        library
            .set_asset_tag(std::slice::from_ref(&asset), family.id, true)
            .unwrap();
        let assignment = library
            .asset_tag_assignments(std::slice::from_ref(&asset))
            .unwrap();
        assert_eq!(
            assignment
                .iter()
                .find(|item| item.tag.id == family.id)
                .unwrap()
                .assigned_count,
            1
        );
        let impact = library.delete_custom_tag(people.id).unwrap();
        assert_eq!(
            impact,
            TagDeleteImpact {
                tag_count: 2,
                asset_count: 1
            }
        );
        assert!(library.custom_tags().unwrap().is_empty());
    }

    #[test]
    fn rejects_hierarchy_cycles_and_moves_asset_state() {
        let library = Library::in_memory().unwrap();
        let root = library.create_custom_tag(None, "Root").unwrap();
        let child = library.create_custom_tag(Some(root.id), "Child").unwrap();
        assert!(matches!(
            library.update_custom_tag(root.id, Some(child.id), "Root"),
            Err(LibraryError::TagHierarchyCycle)
        ));
        let source = PathBuf::from("/photos/a.jpg");
        let destination = PathBuf::from("/photos/b.jpg");
        library
            .set_asset_tag(std::slice::from_ref(&source), child.id, true)
            .unwrap();
        library.move_asset_tag_state(&source, &destination).unwrap();
        let assignment = library
            .asset_tag_assignments(&[source, destination])
            .unwrap();
        assert_eq!(
            assignment
                .iter()
                .find(|item| item.tag.id == child.id)
                .unwrap()
                .assigned_count,
            1
        );
    }

    #[test]
    fn stale_sync_completion_preserves_a_newer_delete_request() {
        let library = Library::in_memory().unwrap();
        let tag = library.create_custom_tag(None, "Temporary").unwrap();
        let asset = PathBuf::from("/photos/a.jpg");
        library
            .set_asset_tag(std::slice::from_ref(&asset), tag.id, true)
            .unwrap();
        let stale = library.tag_xmp_payload(&asset).unwrap();

        library.delete_custom_tag(tag.id).unwrap();
        library
            .complete_tag_xmp_sync(&asset, &stale.subjects, &stale.hierarchical)
            .unwrap();

        let pending = library.pending_tag_sync_paths().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0], asset);
        let delete = library.tag_xmp_payload(&asset).unwrap();
        assert!(delete.subjects.is_empty());
        assert!(delete.hierarchical.is_empty());
        assert_eq!(delete.previous_subjects, ["Temporary"]);
        assert_eq!(delete.previous_hierarchical, ["Temporary"]);

        library
            .complete_tag_xmp_sync(&asset, &delete.subjects, &delete.hierarchical)
            .unwrap();
        assert!(library.pending_tag_sync_paths().unwrap().is_empty());
    }

    #[test]
    fn imports_sidecar_tags_without_duplicating_hierarchical_leaves() {
        let library = Library::in_memory().unwrap();
        let asset = PathBuf::from("/photos/a.jpg");
        library
            .import_sidecar_tags(
                &asset,
                &["Family".into(), "Standalone".into()],
                &["People|Family".into()],
            )
            .unwrap();

        let mut assigned = library
            .asset_tag_assignments(std::slice::from_ref(&asset))
            .unwrap()
            .into_iter()
            .filter(|assignment| assignment.assigned_count > 0)
            .map(|assignment| assignment.tag.path)
            .collect::<Vec<_>>();
        assigned.sort();
        assert_eq!(assigned, ["People|Family", "Standalone"]);
        let state = library.tag_xmp_payload(&asset).unwrap();
        assert_eq!(state.previous_subjects, ["Family", "Standalone"]);
        assert_eq!(state.previous_hierarchical, ["People|Family"]);

        library.import_sidecar_tags(&asset, &[], &[]).unwrap();
        assert!(
            library
                .asset_tag_assignments(std::slice::from_ref(&asset))
                .unwrap()
                .into_iter()
                .all(|assignment| assignment.assigned_count == 0)
        );
    }

    #[test]
    fn sidecar_import_does_not_override_a_pending_database_write() {
        let library = Library::in_memory().unwrap();
        let asset = PathBuf::from("/photos/a.jpg");
        let tag = library.create_custom_tag(None, "Keep").unwrap();
        library
            .set_asset_tag(std::slice::from_ref(&asset), tag.id, true)
            .unwrap();

        library.import_sidecar_tags(&asset, &[], &[]).unwrap();

        let assignment = library
            .asset_tag_assignments(&[asset])
            .unwrap()
            .into_iter()
            .find(|assignment| assignment.tag.id == tag.id)
            .unwrap();
        assert_eq!(assignment.assigned_count, 1);
    }
    #[test]
    fn batch_assignments_preserve_paths_and_follow_tag_changes() {
        let library = Library::in_memory().unwrap();
        let paths = [PathBuf::from("/a.jpg"), PathBuf::from("/b.jpg")];
        let tag = library.create_custom_tag(None, "Before").unwrap();
        library.set_asset_tag(&paths[..1], tag.id, true).unwrap();
        let rows = library.asset_tag_assignments_by_path(&paths).unwrap();
        assert_eq!(rows[0].path, paths[0]);
        assert_eq!(rows[0].assignments[0].assigned_count, 1);
        assert!(rows[1].assignments.is_empty());
        library.update_custom_tag(tag.id, None, "After").unwrap();
        assert_eq!(
            library.asset_tag_assignments_by_path(&paths).unwrap()[0].assignments[0]
                .tag
                .name,
            "After"
        );
        library.set_asset_tag(&paths[..1], tag.id, false).unwrap();
        assert!(
            library
                .asset_tag_assignments_by_path(&paths)
                .unwrap()
                .iter()
                .all(|row| row.assignments.is_empty())
        );
        assert!(
            library
                .asset_tag_assignments_by_path(&[])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn sidecar_reconciliation_preserves_manual_source_and_requeues_xmp() {
        let library = Library::in_memory().unwrap();
        let asset = std::path::PathBuf::from("/photos/a.jpg");
        let tag = library.create_custom_tag(None, "Keeper").unwrap();
        library
            .set_asset_tag(std::slice::from_ref(&asset), tag.id, true)
            .unwrap();
        let payload = library.tag_xmp_payload(&asset).unwrap();
        library
            .complete_tag_xmp_sync(&asset, &payload.subjects, &payload.hierarchical)
            .unwrap();
        assert!(library.pending_tag_sync_paths().unwrap().is_empty());

        library.import_sidecar_tags(&asset, &[], &[]).unwrap();
        let assignments = library
            .asset_tag_assignments(std::slice::from_ref(&asset))
            .unwrap();
        assert_eq!(
            assignments
                .iter()
                .find(|row| row.tag.id == tag.id)
                .unwrap()
                .assigned_count,
            1
        );
        assert_eq!(
            library.pending_tag_sync_paths().unwrap(),
            vec![asset.clone()]
        );
        let sources: Vec<String> = library
            .write()
            .prepare("SELECT source_kind FROM asset_tag_sources WHERE asset_path=?1 AND tag_id=?2")
            .unwrap()
            .query_map(params![asset.to_string_lossy(), tag.id], |row| {
                row.get("source_kind")
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(sources, ["manual"]);
    }

    #[test]
    fn reopening_migrates_unattributed_assignments_as_legacy() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let path = std::path::PathBuf::from("/photos/legacy.jpg");
        let library = Library::open(&database).unwrap();
        let tag = library.create_custom_tag(None, "Legacy").unwrap();
        library
            .write()
            .execute(
                "INSERT INTO asset_tags(asset_path,tag_id) VALUES (?1,?2)",
                params![path.to_string_lossy(), tag.id],
            )
            .unwrap();
        drop(library);
        let reopened = Library::open(&database).unwrap();
        let source: String = reopened
            .write()
            .query_row(
                "SELECT source_kind FROM asset_tag_sources WHERE asset_path=?1 AND tag_id=?2",
                params![path.to_string_lossy(), tag.id],
                |row| row.get("source_kind"),
            )
            .unwrap();
        assert_eq!(source, "legacy");
        reopened
            .set_asset_tag(std::slice::from_ref(&path), tag.id, false)
            .unwrap();
        assert_eq!(
            reopened.asset_tag_assignments(&[path]).unwrap()[0].assigned_count,
            0
        );
    }

    #[test]
    fn person_sources_require_last_belonging_instance_and_preserve_manual_tag() {
        use oxy_domain::{PersonReviewDecision, SetPersonReview};
        let library = Library::in_memory().unwrap();
        let folder = PathBuf::from("/photos");
        let image = folder.join("pair.jpg");
        let tag = library.create_custom_tag(None, "Alex").unwrap();
        let first = library.create_folder_person(&folder, "first").unwrap();
        let second = library.create_folder_person(&folder, "second").unwrap();
        {
            let connection = library.write();
            connection.execute("INSERT INTO historical_people(id,display_name,reference_asset_path,reference_source_revision) VALUES ('history','Alex',?1,'10:20')", [image.to_string_lossy()]).unwrap();
            for subject in [&first.id, &second.id] {
                connection.execute("INSERT INTO folder_historical_links(subject_id,historical_person_id) VALUES (?1,'history')", [subject]).unwrap();
            }
        }
        let mut reviews = Vec::new();
        for (index, subject) in [&first, &second].into_iter().enumerate() {
            let instance = library
                .create_person_instance(&oxy_domain::CreatePersonInstance {
                    folder_path: folder.clone(),
                    asset_path: image.clone(),
                    source_revision: "10:20".into(),
                    face_box: Some([0.1 + index as f64 * 0.4, 0.1, 0.2, 0.2]),
                    body_box: None,
                    request_id: format!("instance-{index}"),
                })
                .unwrap();
            let review = library
                .set_person_review(&SetPersonReview {
                    folder_path: folder.clone(),
                    instance_id: instance.id,
                    subject_id: subject.id.clone(),
                    decision: PersonReviewDecision::Belongs,
                    expected_revision: 0,
                    request_id: format!("belongs-{index}"),
                })
                .unwrap();
            reviews.push(review);
        }
        let input = SetPersonTagLink {
            folder_path: folder.clone(),
            subject_id: first.id.clone(),
            tag_id: Some(tag.id),
            enabled: true,
            expected_revision: 0,
            request_id: "bind-tag".into(),
        };
        let link = library.set_person_tag_link(&input).unwrap();
        assert_eq!(link, library.set_person_tag_link(&input).unwrap());
        assert_eq!(
            library.asset_tag_source_kinds(&image, tag.id).unwrap(),
            ["person"]
        );
        assert_eq!(
            library
                .asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            1
        );
        library
            .set_person_review(&SetPersonReview {
                folder_path: folder.clone(),
                instance_id: reviews[0].instance.id.clone(),
                subject_id: first.id,
                decision: PersonReviewDecision::DoesNotBelong,
                expected_revision: 1,
                request_id: "remove-first".into(),
            })
            .unwrap();
        assert_eq!(
            library
                .asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            1
        );
        let override_input = SetPersonTagOverride {
            folder_path: folder.clone(),
            subject_id: input.subject_id.clone(),
            asset_path: image.clone(),
            suppressed: true,
            expected_revision: 0,
            request_id: "suppress-photo".into(),
        };
        let suppressed = library.set_person_tag_override(&override_input).unwrap();
        assert_eq!(
            suppressed,
            library.set_person_tag_override(&override_input).unwrap()
        );
        assert_eq!(
            library
                .asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            0
        );
        library
            .set_person_tag_override(&SetPersonTagOverride {
                suppressed: false,
                expected_revision: suppressed.revision,
                request_id: "restore-photo".into(),
                ..override_input
            })
            .unwrap();
        assert_eq!(
            library
                .asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            1
        );
        library
            .set_asset_tag(std::slice::from_ref(&image), tag.id, true)
            .unwrap();
        assert_eq!(
            library.asset_tag_source_kinds(&image, tag.id).unwrap(),
            ["manual", "person"]
        );
        library
            .set_person_review(&SetPersonReview {
                folder_path: folder,
                instance_id: reviews[1].instance.id.clone(),
                subject_id: second.id,
                decision: PersonReviewDecision::DoesNotBelong,
                expected_revision: 1,
                request_id: "remove-second".into(),
            })
            .unwrap();
        assert_eq!(
            library
                .asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            1
        );
        library
            .set_asset_tag(std::slice::from_ref(&image), tag.id, false)
            .unwrap();
        assert_eq!(
            library
                .asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            0
        );
        library.delete_custom_tag(tag.id).unwrap();
        let broken = library
            .get_person_tag_link(&input.folder_path, &input.subject_id)
            .unwrap()
            .unwrap();
        assert_eq!(broken.tag_id, None);
        assert!(!broken.enabled);
    }

    #[test]
    fn same_folder_rename_moves_person_facts_with_tag_sources() {
        let library = Library::in_memory().unwrap();
        let folder = PathBuf::from("/photos");
        let source = folder.join("before.jpg");
        let destination = folder.join("after.jpg");
        let tag = library.create_custom_tag(None, "Alex").unwrap();
        let person = library.create_folder_person(&folder, "new-person").unwrap();
        let instance = library
            .create_person_instance(&oxy_domain::CreatePersonInstance {
                folder_path: folder.clone(),
                asset_path: source.clone(),
                source_revision: "10:20".into(),
                face_box: Some([0.1, 0.1, 0.2, 0.2]),
                body_box: None,
                request_id: "new-face".into(),
            })
            .unwrap();
        {
            let connection = library.write();
            connection.execute("INSERT INTO historical_people(id,display_name,reference_asset_path,reference_source_revision) VALUES ('history','Alex',?1,'10:20')",[source.to_string_lossy()]).unwrap();
            connection.execute("INSERT INTO folder_historical_links(subject_id,historical_person_id) VALUES (?1,'history')",[&person.id]).unwrap();
        }
        library
            .set_person_review(&oxy_domain::SetPersonReview {
                folder_path: folder.clone(),
                instance_id: instance.id.clone(),
                subject_id: person.id.clone(),
                decision: oxy_domain::PersonReviewDecision::Belongs,
                expected_revision: 0,
                request_id: "belongs".into(),
            })
            .unwrap();
        library
            .set_person_tag_link(&SetPersonTagLink {
                folder_path: folder.clone(),
                subject_id: person.id,
                tag_id: Some(tag.id),
                enabled: true,
                expected_revision: 0,
                request_id: "bind".into(),
            })
            .unwrap();
        library.move_asset_tag_state(&source, &destination).unwrap();
        assert!(
            library
                .list_person_instances(&folder, &source)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            library
                .list_person_instances(&folder, &destination)
                .unwrap()[0]
                .id,
            instance.id
        );
        assert_eq!(
            library
                .asset_tag_source_kinds(&destination, tag.id)
                .unwrap(),
            ["person"]
        );
        assert!(
            library
                .asset_tag_source_kinds(&source, tag.id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            library.list_historical_people().unwrap()[0].reference_asset_path,
            destination
        );
        let elsewhere = PathBuf::from("/other/after.jpg");
        library
            .move_asset_tag_state(&destination, &elsewhere)
            .unwrap();
        assert!(
            library
                .asset_tag_source_kinds(&elsewhere, tag.id)
                .unwrap()
                .is_empty()
        );
        assert!(
            library
                .list_person_instances(&folder, &destination)
                .unwrap()[0]
                .needs_review
        );
    }
}
