//! The tag vocabulary the user built, the assignments they made, and the XMP
//! state that mirrors them.
//!
//! Tags are facts the user entered, so they are never part of a cache rebuild;
//! only an explicit delete removes them. The tables and their `user` class are
//! declared in `oxy_store::schema::user`, next to the DDL, and every statement
//! that reads or writes them is a function in `oxy_store::repo`. What stays
//! here is what a tag *is*: that the name is unique among siblings, that a tag
//! cannot be moved below itself, and what deleting one cascades into.

use crate::{
    Library, LibraryError,
    cache::features::{forget_asset, rename_asset},
};
use oxy_domain::{
    AssetTagAssignment, AssetTagAssignmentsByPath, CustomTag, CustomTagId, PersonTagLink,
    PersonTagOverride, SetPersonTagLink, SetPersonTagOverride, TagDeleteImpact, TagSyncStatus,
};
use oxy_store::{StoreError, repo};
use std::{collections::HashMap, path::Path};

impl Library {
    pub fn asset_tag_source_kinds(
        &self,
        path: &Path,
        tag_id: CustomTagId,
    ) -> Result<Vec<String>, LibraryError> {
        Ok(repo::tags::source_kinds(
            &self.read_connection(),
            &path.to_string_lossy(),
            tag_id,
        )?)
    }

    pub fn get_person_tag_link(
        &self,
        folder_path: &Path,
        subject_id: &str,
    ) -> Result<Option<PersonTagLink>, LibraryError> {
        Ok(repo::people::person_tag_link(
            &self.read_connection(),
            folder_path,
            subject_id,
        )?)
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
        let historical_id =
            repo::people::historical_person_of_subject(&tx, &input.folder_path, &input.subject_id)?
                .ok_or(LibraryError::MissingPersonRecord)?;
        if let Some((operation, id)) = repo::people::request_result(&tx, &input.request_id)? {
            if operation != "setPersonTagLink" || id != historical_id {
                return Err(LibraryError::PersonConflict);
            }
        } else {
            let revision = repo::people::person_tag_link_revision(&tx, &historical_id)?;
            if revision.unwrap_or(0) != input.expected_revision {
                return Err(LibraryError::PersonConflict);
            }
            if let Some(tag_id) = input.tag_id
                && !repo::tags::tag_exists(&tx, tag_id)?
            {
                return Err(LibraryError::MissingTagParent);
            }
            repo::people::upsert_person_tag_link(&tx, &historical_id, input.tag_id, input.enabled)?;
            for subject in repo::people::subjects_of_historical_person(&tx, &historical_id)? {
                repo::cross::reconcile_person_sources_for_subject(&tx, &subject)?;
            }
            repo::people::record_request_result(
                &tx,
                &input.request_id,
                "setPersonTagLink",
                &historical_id,
            )?;
        }
        let result = repo::people::person_tag_link_of(&tx, &historical_id)?
            .ok_or(LibraryError::MissingPersonRecord)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn get_person_tag_override(
        &self,
        folder_path: &Path,
        subject_id: &str,
        asset_path: &Path,
    ) -> Result<Option<PersonTagOverride>, LibraryError> {
        Ok(repo::people::person_tag_override(
            &self.read_connection(),
            folder_path,
            subject_id,
            asset_path,
        )?)
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
        let historical_id =
            repo::people::historical_person_of_subject(&tx, &input.folder_path, &input.subject_id)?
                .ok_or(LibraryError::MissingPersonRecord)?;
        let request_entity = format!("{}:{}", historical_id, input.asset_path.display());
        let asset_path = input.asset_path.to_string_lossy();
        if let Some((operation, entity)) = repo::people::request_result(&tx, &input.request_id)? {
            if operation != "setPersonTagOverride" || entity != request_entity {
                return Err(LibraryError::PersonConflict);
            }
        } else {
            let revision =
                repo::people::person_tag_override_revision(&tx, &historical_id, &asset_path)?;
            if revision.unwrap_or(0) != input.expected_revision {
                return Err(LibraryError::PersonConflict);
            }
            repo::people::upsert_person_tag_override(
                &tx,
                &historical_id,
                &asset_path,
                input.suppressed,
            )?;
            for subject in repo::people::subjects_of_historical_person(&tx, &historical_id)? {
                repo::cross::reconcile_person_source_for_asset(&tx, &subject, &asset_path)?;
            }
            repo::people::record_request_result(
                &tx,
                &input.request_id,
                "setPersonTagOverride",
                &request_entity,
            )?;
        }
        let result = repo::people::person_tag_override_of(&tx, &historical_id, &asset_path)?
            .ok_or(LibraryError::MissingPersonRecord)?;
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

/// Turns a rejected sibling-name constraint into the domain error it means.
///
/// The unique index on `(parent, name_key)` is the only constraint a tag write
/// can legitimately hit; anything else is a real storage failure.
fn map_constraint(error: StoreError) -> LibraryError {
    if let StoreError::Sqlite(rusqlite::Error::SqliteFailure(value, _)) = &error
        && value.code == rusqlite::ErrorCode::ConstraintViolation
    {
        return LibraryError::DuplicateTagName;
    }
    LibraryError::Store(error)
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
        let matches = repo::tags::assets_matching_tags(
            &self.read_connection(),
            &selected,
            &paths,
            query.tag_match == oxy_domain::TagMatchMode::Any,
        )?;
        Ok(assets
            .iter()
            .filter(|asset| matches.contains(asset.path.to_string_lossy().as_ref()))
            .cloned()
            .collect())
    }

    pub fn custom_tags(&self) -> Result<Vec<CustomTag>, LibraryError> {
        Ok(repo::tags::list_tags(&self.read_connection())?)
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
            && !repo::tags::tag_exists(&transaction, parent_id)?
        {
            return Err(LibraryError::MissingTagParent);
        }
        let sort_order = repo::tags::next_sort_order(&transaction, parent_id)?;
        let id = repo::tags::insert_tag(&transaction, parent_id, &name, &name_key, sort_order)
            .map_err(map_constraint)?;
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
        let (current_parent_id, current_sort_order) = repo::tags::tag_position(&transaction, id)?
            .ok_or(LibraryError::MissingTagParent)?;
        if let Some(parent_id) = parent_id {
            if !repo::tags::tag_exists(&transaction, parent_id)? {
                return Err(LibraryError::MissingTagParent);
            }
            if repo::tags::subtree_contains(&transaction, id, parent_id)? {
                return Err(LibraryError::TagHierarchyCycle);
            }
        }
        let affected = repo::tags::descendant_asset_paths(&transaction, id)?;
        let sort_order = if current_parent_id == parent_id {
            current_sort_order
        } else {
            repo::tags::next_sort_order_excluding(&transaction, parent_id, id)?
        };
        repo::tags::update_tag(&transaction, id, parent_id, &name, &name_key, sort_order)
            .map_err(map_constraint)?;
        repo::tags::enqueue_sync(&transaction, &affected)?;
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
        let (tag_count, asset_count) =
            repo::tags::delete_impact(&self.read_connection(), id)?;
        Ok(TagDeleteImpact {
            tag_count,
            asset_count,
        })
    }

    pub fn delete_custom_tag(&self, id: CustomTagId) -> Result<TagDeleteImpact, LibraryError> {
        let impact = self.custom_tag_delete_impact(id)?;
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let affected = repo::tags::descendant_asset_paths(&transaction, id)?;
        repo::tags::delete_tag(&transaction, id)?;
        repo::tags::enqueue_sync(&transaction, &affected)?;
        transaction.commit()?;
        Ok(impact)
    }

    pub fn asset_tag_assignments(
        &self,
        paths: &[std::path::PathBuf],
    ) -> Result<Vec<AssetTagAssignment>, LibraryError> {
        let tags = self.custom_tags()?;
        let path_strings = selection_paths(paths);
        let mut assigned = HashMap::<CustomTagId, usize>::new();
        for (_, tag_id) in
            repo::tags::assigned_tag_ids(&self.read_connection(), &path_strings)?
        {
            *assigned.entry(tag_id).or_default() += 1;
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
        let mut assigned = HashMap::<String, std::collections::HashSet<CustomTagId>>::new();
        for (path, tag_id) in
            repo::tags::assigned_tag_ids(&self.read_connection(), &selection_paths(paths))?
        {
            assigned.entry(path).or_default().insert(tag_id);
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
        if !repo::tags::tag_exists(&transaction, tag_id)? {
            return Err(LibraryError::MissingTagParent);
        }
        let path_strings = selection_paths(paths);
        for path in &path_strings {
            // A hand assignment drops the legacy inference and records a manual
            // source; removing it drops both. Either way the effective set is
            // then re-derived from the sources that remain.
            repo::tags::replace_manual_source(&transaction, path, tag_id, assigned)?;
            repo::tags::reconcile_effective(&transaction, path, tag_id)?;
        }
        repo::tags::enqueue_sync(&transaction, &path_strings)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_tag_sync_paths(&self) -> Result<Vec<std::path::PathBuf>, LibraryError> {
        Ok(repo::tags::pending_sync_paths(&self.read_connection())?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub fn tag_xmp_payload(&self, path: &Path) -> Result<TagXmpPayload, LibraryError> {
        let connection = self.read_connection();
        let asset_path = path.to_string_lossy();
        let tags = repo::tags::assigned_tag_paths(&connection, &asset_path)?;
        let subjects = tags
            .iter()
            .filter_map(|value| value.rsplit('|').next().map(str::to_owned))
            .collect::<Vec<_>>();
        let (previous_subjects, previous_hierarchical) =
            repo::tags::xmp_state(&connection, &asset_path)?.map_or_else(
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
        let asset_path = path.to_string_lossy();
        repo::tags::put_xmp_state(
            &connection,
            &asset_path,
            &serde_json::to_string(subjects)?,
            &serde_json::to_string(hierarchical)?,
        )?;
        let current_hierarchical = repo::tags::assigned_tag_paths(&connection, &asset_path)?;
        let current_subjects = current_hierarchical
            .iter()
            .filter_map(|value| value.rsplit('|').next().map(str::to_owned))
            .collect::<Vec<_>>();
        if subjects == current_subjects && hierarchical == current_hierarchical {
            repo::tags::delete_sync_entry(&connection, &asset_path)?;
        }
        Ok(())
    }

    pub fn fail_tag_xmp_sync(&self, path: &Path, error: &str) -> Result<(), LibraryError> {
        repo::tags::record_sync_failure(&self.write(), &path.to_string_lossy(), error)?;
        Ok(())
    }

    pub fn tag_sync_status(&self) -> Result<TagSyncStatus, LibraryError> {
        let (pending_count, failed_count, last_error) =
            repo::tags::sync_status(&self.read_connection())?;
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
        let asset_path = path.to_string_lossy();
        if repo::tags::has_pending_sync(&self.write(), &asset_path)? {
            return Ok(());
        }

        let mut desired_tag_ids = std::collections::HashSet::new();
        for value in hierarchical {
            let mut parent_id = None;
            for segment in value.split('|') {
                let (name, name_key) = normalize_name(segment)?;
                let existing = repo::tags::tag_id_at(&self.write(), parent_id, &name_key)?;
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
            let existing = repo::tags::tag_id_at(&self.write(), None, &name_key)?;
            let tag_id = match existing {
                Some(id) => id,
                None => self.create_custom_tag(None, &name)?.id,
            };
            desired_tag_ids.insert(tag_id);
        }

        let mut connection = self.write();
        let transaction = connection.transaction()?;
        for tag_id in repo::tags::source_tag_ids_for_kind(&transaction, &asset_path, "sidecar")? {
            if !desired_tag_ids.contains(&tag_id) {
                repo::tags::delete_source_of_kind(&transaction, &asset_path, tag_id, "sidecar")?;
                repo::tags::reconcile_effective(&transaction, &asset_path, tag_id)?;
            }
        }
        for tag_id in &desired_tag_ids {
            repo::tags::insert_source(&transaction, &asset_path, *tag_id, "sidecar", "")?;
            repo::tags::reconcile_effective(&transaction, &asset_path, *tag_id)?;
        }
        if repo::tags::effective_tag_ids(&transaction, &asset_path)? != desired_tag_ids {
            repo::tags::enqueue_sync(&transaction, &[asset_path.into_owned()])?;
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
        // A move inside the same folder keeps the person source: it is still
        // the same photo of the same person. A copy, or a move to another
        // folder, is a different asset and must be reviewed on its own.
        let same_folder_move = !copy && source.parent() == destination.parent();
        let source = source.to_string_lossy();
        let destination = destination.to_string_lossy();
        repo::tags::copy_source_rows(&transaction, &source, &destination, same_folder_move)?;
        repo::tags::copy_effective_rows(&transaction, &source, &destination, same_folder_move)?;
        if copy {
            repo::tags::enqueue_sync(&transaction, &[destination.into_owned()])?;
        } else {
            if same_folder_move {
                // The feature cache still describes the same photo, so point it
                // at the new path instead of throwing the vectors away.
                rename_asset(&transaction, &source, &destination)?;
                repo::people::repoint_asset_rows(&transaction, &source, &destination)?;
            } else {
                forget_asset(&transaction, &source)?;
                repo::people::require_review_for_asset(&transaction, &source)?;
            }
            repo::tags::move_xmp_state(&transaction, &source, &destination)?;
            repo::tags::forget_asset_rows(&transaction, &source)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn remove_asset_tag_state(&self, path: &Path) -> Result<(), LibraryError> {
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let path = path.to_string_lossy();
        forget_asset(&transaction, &path)?;
        repo::tags::forget_asset_rows(&transaction, &path)?;
        transaction.commit()?;
        Ok(())
    }
}

/// The paths of a selection, in the form the assignment tables store.
fn selection_paths(paths: &[std::path::PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
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
