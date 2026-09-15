use crate::{Library, LibraryError};
use oxy_domain::{
    AssetTagAssignment, AssetTagAssignmentsByPath, CustomTag, CustomTagId, TagDeleteImpact,
    TagSyncStatus,
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::{collections::HashMap, path::Path};

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
                |row| row.get::<_, String>(0),
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
                    row.get::<_, CustomTagId>(0)?,
                    row.get::<_, Option<CustomTagId>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
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
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        if let Some(parent_id) = parent_id
            && !tag_exists(&transaction, parent_id)?
        {
            return Err(LibraryError::MissingTagParent);
        }
        let sort_order = transaction.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM custom_tags
             WHERE parent_id IS ?1",
            params![parent_id],
            |row| row.get::<_, i64>(0),
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
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let (current_parent_id, current_sort_order) = transaction
            .query_row(
                "SELECT parent_id, sort_order FROM custom_tags WHERE id=?1",
                params![id],
                |row| Ok((row.get::<_, Option<CustomTagId>>(0)?, row.get::<_, i64>(1)?)),
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
                 ) SELECT EXISTS(SELECT 1 FROM descendants WHERE id = ?2)",
                params![id, parent_id],
                |row| row.get::<_, bool>(0),
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
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM custom_tags
                 WHERE parent_id IS ?1 AND id != ?2",
                params![parent_id, id],
                |row| row.get::<_, i64>(0),
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
             ) SELECT COUNT(DISTINCT descendants.id), COUNT(DISTINCT asset_tags.asset_path)
               FROM descendants LEFT JOIN asset_tags ON asset_tags.tag_id = descendants.id",
            params![id],
            |row| Ok((row.get::<_, usize>(0)?, row.get::<_, usize>(1)?)),
        )?;
        Ok(TagDeleteImpact {
            tag_count,
            asset_count,
        })
    }

    pub fn delete_custom_tag(&self, id: CustomTagId) -> Result<TagDeleteImpact, LibraryError> {
        let impact = self.custom_tag_delete_impact(id)?;
        let mut connection = self.connection.lock();
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
            for id in statement.query_map(params![path.to_string_lossy()], |row| row.get(0))? {
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
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, CustomTagId>(1)?)),
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
        let mut connection = self.connection.lock();
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
                    "INSERT OR IGNORE INTO asset_tags(asset_path, tag_id) VALUES (?1, ?2)",
                    params![path, tag_id],
                )?;
            } else {
                transaction.execute(
                    "DELETE FROM asset_tags WHERE asset_path = ?1 AND tag_id = ?2",
                    params![path, tag_id],
                )?;
            }
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
            .query_map([], |row| row.get::<_, String>(0))?
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
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
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
        let connection = self.connection.lock();
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
        self.connection.lock().execute(
            "UPDATE tag_xmp_sync_queue SET attempt_count=attempt_count + 1, last_error=?2
             WHERE asset_path=?1",
            params![path.to_string_lossy(), error],
        )?;
        Ok(())
    }

    pub fn tag_sync_status(&self) -> Result<TagSyncStatus, LibraryError> {
        let connection = self.read_connection();
        let (pending_count, failed_count) = connection.query_row(
            "SELECT COUNT(*), COUNT(last_error) FROM tag_xmp_sync_queue",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let last_error = connection
            .query_row(
                "SELECT last_error FROM tag_xmp_sync_queue WHERE last_error IS NOT NULL
                 ORDER BY requested_at DESC LIMIT 1",
                [],
                |row| row.get(0),
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
        let has_pending_write = self.connection.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM tag_xmp_sync_queue WHERE asset_path = ?1)",
            params![path.to_string_lossy()],
            |row| row.get::<_, bool>(0),
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
                    .connection
                    .lock()
                    .query_row(
                        "SELECT id FROM custom_tags WHERE parent_id IS ?1 AND name_key = ?2",
                        params![parent_id, name_key],
                        |row| row.get::<_, CustomTagId>(0),
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
                .connection
                .lock()
                .query_row(
                    "SELECT id FROM custom_tags WHERE parent_id IS NULL AND name_key = ?1",
                    params![name_key],
                    |row| row.get::<_, CustomTagId>(0),
                )
                .optional()?;
            let tag_id = match existing {
                Some(id) => id,
                None => self.create_custom_tag(None, &name)?.id,
            };
            desired_tag_ids.insert(tag_id);
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let existing_tag_ids = {
            let mut statement =
                transaction.prepare("SELECT tag_id FROM asset_tags WHERE asset_path = ?1")?;
            statement
                .query_map(params![path.to_string_lossy()], |row| {
                    row.get::<_, CustomTagId>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for tag_id in existing_tag_ids {
            if !desired_tag_ids.contains(&tag_id) {
                transaction.execute(
                    "DELETE FROM asset_tags WHERE asset_path = ?1 AND tag_id = ?2",
                    params![path.to_string_lossy(), tag_id],
                )?;
            }
        }
        for tag_id in desired_tag_ids {
            transaction.execute(
                "INSERT OR IGNORE INTO asset_tags(asset_path, tag_id) VALUES (?1, ?2)",
                params![path.to_string_lossy(), tag_id],
            )?;
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
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO asset_tags(asset_path, tag_id)
             SELECT ?2, tag_id FROM asset_tags WHERE asset_path=?1",
            params![source.to_string_lossy(), destination.to_string_lossy()],
        )?;
        if copy {
            enqueue_paths(&transaction, &[destination.to_string_lossy().into_owned()])?;
        } else {
            transaction.execute(
                "UPDATE OR REPLACE asset_tag_xmp_state SET asset_path=?2 WHERE asset_path=?1",
                params![source.to_string_lossy(), destination.to_string_lossy()],
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
        let connection = self.connection.lock();
        let path = path.to_string_lossy();
        connection.execute("DELETE FROM asset_tags WHERE asset_path=?1", params![path])?;
        connection.execute(
            "DELETE FROM asset_tag_xmp_state WHERE asset_path=?1",
            params![path],
        )?;
        connection.execute(
            "DELETE FROM tag_xmp_sync_queue WHERE asset_path=?1",
            params![path],
        )?;
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
        .query_map(params![id], |row| row.get(0))?
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
                    row.get::<_, CustomTagId>(0)?,
                    row.get::<_, Option<CustomTagId>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let all_names = {
        let mut statement = connection.prepare("SELECT id, parent_id, name FROM custom_tags")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, CustomTagId>(0)?,
                    (
                        row.get::<_, Option<CustomTagId>>(1)?,
                        row.get::<_, String>(2)?,
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
}
