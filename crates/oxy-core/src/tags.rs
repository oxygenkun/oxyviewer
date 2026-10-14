//! User tag vocabulary, independent assignment sources and durable XMP mirrors.
//! Schema and statements live beside these rules. Cross-feature asset changes
//! and person contributions use named application workflows in one transaction.

pub(crate) mod repo;
pub(crate) mod schema;
pub mod types;
pub mod xmp_sync;

use crate::browse::types::{AssetQuery, AssetSummary, TagMatchMode};
use crate::storage::{Store, StoreError};
use crate::tags::types::{
    AssetTagAssignment, AssetTagAssignmentsByPath, CustomTag, CustomTagId, TagDeleteImpact,
    TagSyncStatus,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TagError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("tag name must not be empty or contain '|'")]
    InvalidTagName,
    #[error("tag parent does not exist")]
    MissingTagParent,
    #[error("a tag cannot be moved below itself")]
    TagHierarchyCycle,
    #[error("a tag with this name already exists at this level")]
    DuplicateTagName,
}

/// The tag vocabulary and the assignments over one store.
///
/// The store is shared with the crates that own the other domains, so the
/// application builds each of them over the same file rather than one wrapping
/// the others.
pub struct Tags {
    store: Arc<Store>,
}

impl Tags {
    pub(crate) fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn asset_tag_source_kinds(
        &self,
        path: &Path,
        tag_id: CustomTagId,
    ) -> Result<Vec<String>, TagError> {
        Ok(crate::tags::repo::source_kinds(
            &self.store.read(),
            &path.to_string_lossy(),
            tag_id,
        )?)
    }

    /// Match explicit assignments against selected subtrees in one SQLite read.
    /// Only the supplied directory snapshot participates; no index or metadata
    /// is needed.
    pub fn filter_assets_by_tags(
        &self,
        assets: &[AssetSummary],
        query: &AssetQuery,
    ) -> Result<Vec<AssetSummary>, TagError> {
        if query.tag_ids.is_empty() {
            return Ok(assets.to_vec());
        }
        let selected = serde_json::to_string(&query.tag_ids)?;
        let paths =
            serde_json::to_string(&assets.iter().map(|asset| &asset.path).collect::<Vec<_>>())?;
        let matches = crate::tags::repo::assets_matching_tags(
            &self.store.read(),
            &selected,
            &paths,
            query.tag_match == TagMatchMode::Any,
        )?;
        Ok(assets
            .iter()
            .filter(|asset| matches.contains(asset.path.to_string_lossy().as_ref()))
            .cloned()
            .collect())
    }

    pub fn custom_tags(&self) -> Result<Vec<CustomTag>, TagError> {
        Ok(crate::tags::repo::list_tags(&self.store.read())?)
    }

    pub fn create_custom_tag(
        &self,
        parent_id: Option<CustomTagId>,
        name: &str,
    ) -> Result<CustomTag, TagError> {
        let (name, name_key) = normalize_name(name)?;
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        if let Some(parent_id) = parent_id
            && !crate::tags::repo::tag_exists(&transaction, parent_id)?
        {
            return Err(TagError::MissingTagParent);
        }
        let sort_order = crate::tags::repo::next_sort_order(&transaction, parent_id)?;
        let id =
            crate::tags::repo::insert_tag(&transaction, parent_id, &name, &name_key, sort_order)
                .map_err(map_constraint)?;
        transaction.commit().map_err(StoreError::from)?;
        drop(connection);
        self.custom_tags()?
            .into_iter()
            .find(|tag| tag.id == id)
            .ok_or(TagError::MissingTagParent)
    }

    pub fn update_custom_tag(
        &self,
        id: CustomTagId,
        parent_id: Option<CustomTagId>,
        name: &str,
    ) -> Result<CustomTag, TagError> {
        let (name, name_key) = normalize_name(name)?;
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        let (current_parent_id, current_sort_order) =
            crate::tags::repo::tag_position(&transaction, id)?.ok_or(TagError::MissingTagParent)?;
        if let Some(parent_id) = parent_id {
            if !crate::tags::repo::tag_exists(&transaction, parent_id)? {
                return Err(TagError::MissingTagParent);
            }
            if crate::tags::repo::subtree_contains(&transaction, id, parent_id)? {
                return Err(TagError::TagHierarchyCycle);
            }
        }
        let affected = crate::tags::repo::descendant_asset_paths(&transaction, id)?;
        let sort_order = if current_parent_id == parent_id {
            current_sort_order
        } else {
            crate::tags::repo::next_sort_order_excluding(&transaction, parent_id, id)?
        };
        crate::tags::repo::update_tag(&transaction, id, parent_id, &name, &name_key, sort_order)
            .map_err(map_constraint)?;
        crate::tags::repo::enqueue_sync(&transaction, &affected)?;
        transaction.commit().map_err(StoreError::from)?;
        drop(connection);
        self.custom_tags()?
            .into_iter()
            .find(|tag| tag.id == id)
            .ok_or(TagError::MissingTagParent)
    }

    pub fn custom_tag_delete_impact(&self, id: CustomTagId) -> Result<TagDeleteImpact, TagError> {
        let (tag_count, asset_count) = crate::tags::repo::delete_impact(&self.store.read(), id)?;
        Ok(TagDeleteImpact {
            tag_count,
            asset_count,
        })
    }

    pub fn delete_custom_tag(&self, id: CustomTagId) -> Result<TagDeleteImpact, TagError> {
        let impact = self.custom_tag_delete_impact(id)?;
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        let affected = crate::tags::repo::descendant_asset_paths(&transaction, id)?;
        crate::tags::repo::delete_tag(&transaction, id)?;
        crate::tags::repo::enqueue_sync(&transaction, &affected)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(impact)
    }

    pub fn asset_tag_assignments(
        &self,
        paths: &[PathBuf],
    ) -> Result<Vec<AssetTagAssignment>, TagError> {
        let tags = self.custom_tags()?;
        let path_strings = selection_paths(paths);
        let mut assigned = HashMap::<CustomTagId, usize>::new();
        for (_, tag_id) in crate::tags::repo::assigned_tag_ids(&self.store.read(), &path_strings)? {
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
        paths: &[PathBuf],
    ) -> Result<Vec<AssetTagAssignmentsByPath>, TagError> {
        let tags = self.custom_tags()?;
        let mut assigned = HashMap::<String, std::collections::HashSet<CustomTagId>>::new();
        for (path, tag_id) in
            crate::tags::repo::assigned_tag_ids(&self.store.read(), &selection_paths(paths))?
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
        paths: &[PathBuf],
        tag_id: CustomTagId,
        assigned: bool,
    ) -> Result<(), TagError> {
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        if !crate::tags::repo::tag_exists(&transaction, tag_id)? {
            return Err(TagError::MissingTagParent);
        }
        let path_strings = selection_paths(paths);
        for path in &path_strings {
            // A hand assignment records a manual source; removing it drops that
            // source. Either way the effective set is
            // then re-derived from the sources that remain.
            crate::tags::repo::replace_manual_source(&transaction, path, tag_id, assigned)?;
            crate::tags::repo::reconcile_effective(&transaction, path, tag_id)?;
        }
        crate::tags::repo::enqueue_sync(&transaction, &path_strings)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(())
    }

    pub fn pending_tag_sync_paths(&self) -> Result<Vec<PathBuf>, TagError> {
        Ok(crate::tags::repo::pending_sync_paths(&self.store.read())?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub fn tag_xmp_payload(&self, path: &Path) -> Result<TagXmpPayload, TagError> {
        let connection = self.store.read();
        let asset_path = path.to_string_lossy();
        let tags = crate::tags::repo::assigned_tag_paths(&connection, &asset_path)?;
        let subjects = tags
            .iter()
            .filter_map(|value| value.rsplit('|').next().map(str::to_owned))
            .collect::<Vec<_>>();
        let (previous_subjects, previous_hierarchical) =
            crate::tags::repo::xmp_state(&connection, &asset_path)?.map_or_else(
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
    ) -> Result<(), TagError> {
        let connection = self.store.write();
        let asset_path = path.to_string_lossy();
        crate::tags::repo::put_xmp_state(
            &connection,
            &asset_path,
            &serde_json::to_string(subjects)?,
            &serde_json::to_string(hierarchical)?,
        )?;
        let current_hierarchical = crate::tags::repo::assigned_tag_paths(&connection, &asset_path)?;
        let current_subjects = current_hierarchical
            .iter()
            .filter_map(|value| value.rsplit('|').next().map(str::to_owned))
            .collect::<Vec<_>>();
        if subjects == current_subjects && hierarchical == current_hierarchical {
            crate::tags::repo::delete_sync_entry(&connection, &asset_path)?;
        }
        Ok(())
    }

    pub fn fail_tag_xmp_sync(&self, path: &Path, error: &str) -> Result<(), TagError> {
        crate::tags::repo::record_sync_failure(
            &self.store.write(),
            &path.to_string_lossy(),
            error,
        )?;
        Ok(())
    }

    pub fn tag_sync_status(&self) -> Result<TagSyncStatus, TagError> {
        let (pending_count, failed_count, last_error) =
            crate::tags::repo::sync_status(&self.store.read())?;
        Ok(TagSyncStatus {
            pending_count,
            failed_count,
            last_error,
        })
    }

    /// Applies the keywords a sidecar already carries.
    ///
    /// The sidecar is the other machine's copy of the user's words, so it may
    /// introduce tags the vocabulary does not have yet. A pending mirror write
    /// for this asset wins: the database is the newer statement, and importing
    /// over it would undo an edit the user just made.
    pub fn import_sidecar_tags(
        &self,
        path: &Path,
        subjects: &[String],
        hierarchical: &[String],
    ) -> Result<(), TagError> {
        let asset_path = path.to_string_lossy();
        if crate::tags::repo::has_pending_sync(&self.store.write(), &asset_path)? {
            return Ok(());
        }

        let mut desired_tag_ids = std::collections::HashSet::new();
        for value in hierarchical {
            let mut parent_id = None;
            for segment in value.split('|') {
                let (name, name_key) = normalize_name(segment)?;
                let existing =
                    crate::tags::repo::tag_id_at(&self.store.write(), parent_id, &name_key)?;
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
            let existing = crate::tags::repo::tag_id_at(&self.store.write(), None, &name_key)?;
            let tag_id = match existing {
                Some(id) => id,
                None => self.create_custom_tag(None, &name)?.id,
            };
            desired_tag_ids.insert(tag_id);
        }

        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        for tag_id in
            crate::tags::repo::source_tag_ids_for_kind(&transaction, &asset_path, "sidecar")?
        {
            if !desired_tag_ids.contains(&tag_id) {
                crate::tags::repo::delete_source_of_kind(
                    &transaction,
                    &asset_path,
                    tag_id,
                    "sidecar",
                )?;
                crate::tags::repo::reconcile_effective(&transaction, &asset_path, tag_id)?;
            }
        }
        for tag_id in &desired_tag_ids {
            crate::tags::repo::insert_source(&transaction, &asset_path, *tag_id, "sidecar", "")?;
            crate::tags::repo::reconcile_effective(&transaction, &asset_path, *tag_id)?;
        }
        if crate::tags::repo::effective_tag_ids(&transaction, &asset_path)? != desired_tag_ids {
            crate::tags::repo::enqueue_sync(&transaction, &[asset_path.into_owned()])?;
        }
        transaction.commit().map_err(StoreError::from)?;
        drop(connection);
        self.complete_tag_xmp_sync(path, subjects, hierarchical)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagXmpPayload {
    pub subjects: Vec<String>,
    pub hierarchical: Vec<String>,
    pub previous_subjects: Vec<String>,
    pub previous_hierarchical: Vec<String>,
}

fn normalize_name(name: &str) -> Result<(String, String), TagError> {
    let name = name.trim();
    if name.is_empty() || name.contains('|') || name.chars().any(char::is_control) {
        return Err(TagError::InvalidTagName);
    }
    Ok((name.to_owned(), name.to_lowercase()))
}

/// Turns a rejected sibling-name constraint into the domain error it means.
///
/// The unique index on `(parent, name_key)` is the only constraint a tag write
/// can legitimately hit; anything else is a real storage failure. Asking the
/// error rather than matching the engine keeps `rusqlite` out of this crate.
fn map_constraint(error: StoreError) -> TagError {
    if error.is_constraint_violation() {
        return TagError::DuplicateTagName;
    }
    TagError::Store(error)
}

/// The paths of a selection, in the form the assignment tables store.
fn selection_paths(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    fn relocate_state(tags: &Tags, source: &Path, destination: &Path) -> Result<(), StoreError> {
        let mut connection = tags.store.write();
        let transaction = connection.transaction()?;
        crate::app::workflows::asset_move::relocate_asset(
            &transaction,
            &source.to_string_lossy(),
            &destination.to_string_lossy(),
            source.parent() == destination.parent(),
        )?;
        transaction.commit()?;
        Ok(())
    }

    use super::*;
    use oxy_media::types::{AssetKind, PickLabel};

    /// A vocabulary over a store nobody else keeps. The cases that drive a
    /// person identity, or that simulate a file written by an earlier build,
    /// reach for [`in_memory_with_store`] instead.
    fn tags() -> Tags {
        Tags::new(Arc::new(Store::in_memory().unwrap()))
    }

    /// The store as well as the vocabulary, so a case can set up a row that no
    /// public API creates.
    fn in_memory_with_store() -> (Tags, Arc<Store>) {
        let store = Arc::new(Store::in_memory().unwrap());
        (Tags::new(Arc::clone(&store)), store)
    }

    #[test]
    fn filters_snapshot_by_subtrees_and_groups_before_paging() {
        use crate::browse::types::{AssetQuery, AssetSummary, TagMatchMode};
        let tags = tags();
        let parent = tags.create_custom_tag(None, "People").unwrap();
        let child = tags.create_custom_tag(Some(parent.id), "Family").unwrap();
        let other = tags.create_custom_tag(None, "Trips").unwrap();
        let same_name = tags.create_custom_tag(Some(other.id), "Family").unwrap();
        let paths = [
            PathBuf::from("/photos/a.jpg"),
            PathBuf::from("/photos/b.jpg"),
            PathBuf::from("/elsewhere/c.jpg"),
        ];
        tags.set_asset_tag(&paths[..1], child.id, true).unwrap();
        tags.set_asset_tag(&paths, other.id, true).unwrap();
        tags.set_asset_tag(&paths[1..2], same_name.id, true)
            .unwrap();
        let assets = paths[..2]
            .iter()
            .enumerate()
            .map(|(index, path)| AssetSummary {
                id: index.to_string(),
                path: path.clone(),
                name: format!("{index}.jpg"),
                extension: "jpg".into(),
                kind: AssetKind::Jpeg,
                size_bytes: 1,
                modified_at_ms: 0,
                has_sidecar: false,
                rating: Some(4),
                color_label: Some("Red".into()),
                pick_label: Some(PickLabel::Accepted),
            })
            .collect::<Vec<_>>();
        let mut query = AssetQuery {
            tag_ids: vec![parent.id],
            ..Default::default()
        };
        assert!(!query.needs_metadata_enrichment());
        assert_eq!(
            tags.filter_assets_by_tags(&assets, &query).unwrap().len(),
            1
        );
        query.tag_ids.push(other.id);
        assert_eq!(
            tags.filter_assets_by_tags(&assets, &query).unwrap().len(),
            1
        );
        query.tag_match = TagMatchMode::Any;
        query.minimum_rating = Some(4);
        query.color_labels = vec!["Red".into()];
        query.pick_labels = vec!["accepted".into()];
        query.page_size = Some(1);
        let matched = tags.filter_assets_by_tags(&assets, &query).unwrap();
        let page = crate::files::page_assets(&matched, &query, 0);
        assert_eq!(page.total, 2);
        assert_eq!(page.items.len(), 1);
        assert_eq!(
            crate::files::page_assets(&matched, &query, 1).items.len(),
            1
        );
        query.tag_ids = vec![child.id];
        tags.update_custom_tag(child.id, Some(other.id), "Renamed")
            .unwrap();
        assert_eq!(
            tags.filter_assets_by_tags(&assets, &query).unwrap().len(),
            1
        );
        query.tag_ids = vec![parent.id];
        assert!(
            tags.filter_assets_by_tags(&assets, &query)
                .unwrap()
                .is_empty()
        );
        query.tag_ids = vec![child.id];
        tags.delete_custom_tag(child.id).unwrap();
        assert!(
            tags.filter_assets_by_tags(&assets, &query)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn manages_hierarchy_assignments_and_subtree_deletion() {
        let tags = tags();
        let people = tags.create_custom_tag(None, "People").unwrap();
        let family = tags.create_custom_tag(Some(people.id), "Family").unwrap();
        assert_eq!(people.name, "People");
        assert_eq!(family.name, "Family");
        assert_eq!(family.path, "People|Family");
        assert!(matches!(
            tags.create_custom_tag(None, "people"),
            Err(TagError::DuplicateTagName)
        ));
        assert!(matches!(
            tags.create_custom_tag(Some(people.id), "family"),
            Err(TagError::DuplicateTagName)
        ));
        let asset = PathBuf::from("/photos/a.jpg");
        tags.set_asset_tag(std::slice::from_ref(&asset), family.id, true)
            .unwrap();
        let assignment = tags
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
        let impact = tags.delete_custom_tag(people.id).unwrap();
        assert_eq!(
            impact,
            TagDeleteImpact {
                tag_count: 2,
                asset_count: 1
            }
        );
        assert!(tags.custom_tags().unwrap().is_empty());
    }

    #[test]
    fn rejects_hierarchy_cycles_and_moves_asset_state() {
        let tags = tags();
        let root = tags.create_custom_tag(None, "Root").unwrap();
        let child = tags.create_custom_tag(Some(root.id), "Child").unwrap();
        assert!(matches!(
            tags.update_custom_tag(root.id, Some(child.id), "Root"),
            Err(TagError::TagHierarchyCycle)
        ));
        let source = PathBuf::from("/photos/a.jpg");
        let destination = PathBuf::from("/photos/b.jpg");
        tags.set_asset_tag(std::slice::from_ref(&source), child.id, true)
            .unwrap();
        relocate_state(&tags, &source, &destination).unwrap();
        let assignment = tags.asset_tag_assignments(&[source, destination]).unwrap();
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
        let tags = tags();
        let tag = tags.create_custom_tag(None, "Temporary").unwrap();
        let asset = PathBuf::from("/photos/a.jpg");
        tags.set_asset_tag(std::slice::from_ref(&asset), tag.id, true)
            .unwrap();
        let stale = tags.tag_xmp_payload(&asset).unwrap();

        tags.delete_custom_tag(tag.id).unwrap();
        tags.complete_tag_xmp_sync(&asset, &stale.subjects, &stale.hierarchical)
            .unwrap();

        let pending = tags.pending_tag_sync_paths().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0], asset);
        let delete = tags.tag_xmp_payload(&asset).unwrap();
        assert!(delete.subjects.is_empty());
        assert!(delete.hierarchical.is_empty());
        assert_eq!(delete.previous_subjects, ["Temporary"]);
        assert_eq!(delete.previous_hierarchical, ["Temporary"]);

        tags.complete_tag_xmp_sync(&asset, &delete.subjects, &delete.hierarchical)
            .unwrap();
        assert!(tags.pending_tag_sync_paths().unwrap().is_empty());
    }

    #[test]
    fn imports_sidecar_tags_without_duplicating_hierarchical_leaves() {
        let tags = tags();
        let asset = PathBuf::from("/photos/a.jpg");
        tags.import_sidecar_tags(
            &asset,
            &["Family".into(), "Standalone".into()],
            &["People|Family".into()],
        )
        .unwrap();

        let mut assigned = tags
            .asset_tag_assignments(std::slice::from_ref(&asset))
            .unwrap()
            .into_iter()
            .filter(|assignment| assignment.assigned_count > 0)
            .map(|assignment| assignment.tag.path)
            .collect::<Vec<_>>();
        assigned.sort();
        assert_eq!(assigned, ["People|Family", "Standalone"]);
        let state = tags.tag_xmp_payload(&asset).unwrap();
        assert_eq!(state.previous_subjects, ["Family", "Standalone"]);
        assert_eq!(state.previous_hierarchical, ["People|Family"]);

        tags.import_sidecar_tags(&asset, &[], &[]).unwrap();
        assert!(
            tags.asset_tag_assignments(std::slice::from_ref(&asset))
                .unwrap()
                .into_iter()
                .all(|assignment| assignment.assigned_count == 0)
        );
    }

    #[test]
    fn sidecar_import_does_not_override_a_pending_database_write() {
        let tags = tags();
        let asset = PathBuf::from("/photos/a.jpg");
        let tag = tags.create_custom_tag(None, "Keep").unwrap();
        tags.set_asset_tag(std::slice::from_ref(&asset), tag.id, true)
            .unwrap();

        tags.import_sidecar_tags(&asset, &[], &[]).unwrap();

        let assignment = tags
            .asset_tag_assignments(&[asset])
            .unwrap()
            .into_iter()
            .find(|assignment| assignment.tag.id == tag.id)
            .unwrap();
        assert_eq!(assignment.assigned_count, 1);
    }

    #[test]
    fn batch_assignments_preserve_paths_and_follow_tag_changes() {
        let tags = tags();
        let paths = [PathBuf::from("/a.jpg"), PathBuf::from("/b.jpg")];
        let tag = tags.create_custom_tag(None, "Before").unwrap();
        tags.set_asset_tag(&paths[..1], tag.id, true).unwrap();
        let rows = tags.asset_tag_assignments_by_path(&paths).unwrap();
        assert_eq!(rows[0].path, paths[0]);
        assert_eq!(rows[0].assignments[0].assigned_count, 1);
        assert!(rows[1].assignments.is_empty());
        tags.update_custom_tag(tag.id, None, "After").unwrap();
        assert_eq!(
            tags.asset_tag_assignments_by_path(&paths).unwrap()[0].assignments[0]
                .tag
                .name,
            "After"
        );
        tags.set_asset_tag(&paths[..1], tag.id, false).unwrap();
        assert!(
            tags.asset_tag_assignments_by_path(&paths)
                .unwrap()
                .iter()
                .all(|row| row.assignments.is_empty())
        );
        assert!(tags.asset_tag_assignments_by_path(&[]).unwrap().is_empty());
    }

    #[test]
    fn sidecar_reconciliation_preserves_manual_source_and_requeues_xmp() {
        let (tags, store) = in_memory_with_store();
        let asset = PathBuf::from("/photos/a.jpg");
        let tag = tags.create_custom_tag(None, "Keeper").unwrap();
        tags.set_asset_tag(std::slice::from_ref(&asset), tag.id, true)
            .unwrap();
        let payload = tags.tag_xmp_payload(&asset).unwrap();
        tags.complete_tag_xmp_sync(&asset, &payload.subjects, &payload.hierarchical)
            .unwrap();
        assert!(tags.pending_tag_sync_paths().unwrap().is_empty());

        tags.import_sidecar_tags(&asset, &[], &[]).unwrap();
        let assignments = tags
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
        assert_eq!(tags.pending_tag_sync_paths().unwrap(), vec![asset.clone()]);
        let sources =
            crate::tags::repo::source_kinds(&store.read(), &asset.to_string_lossy(), tag.id)
                .unwrap();
        assert_eq!(sources, ["manual"]);
    }

    /// The tag side of a person identity: what an identity claims lands as a
    /// `person` source, survives a hand assignment beside it, and is withdrawn
    /// only when the last instance stops belonging.
    #[test]
    fn person_sources_require_last_belonging_instance_and_preserve_manual_tag() {
        use crate::people::People;
        use crate::people::types::{PersonReviewDecision, SetPersonReview};
        use std::path::Path as StdPath;

        let (tags, store) = in_memory_with_store();
        let people = People::new(Arc::clone(&store));
        let folder = PathBuf::from("/photos");
        let image = folder.join("pair.jpg");
        let tag = tags.create_custom_tag(None, "Alex").unwrap();
        let first = people.create_folder_person(&folder, "first").unwrap();
        let second = people.create_folder_person(&folder, "second").unwrap();
        {
            let connection = store.write();
            crate::people::repo::manual::insert_historical_person(
                &connection,
                "history",
                "Alex",
                &image.to_string_lossy(),
                "10:20",
            )
            .unwrap();
            for subject in [&first.id, &second.id] {
                crate::people::repo::manual::upsert_historical_link(
                    &connection,
                    subject,
                    "history",
                )
                .unwrap();
            }
        }
        let mut reviews = Vec::new();
        for (index, subject) in [&first, &second].into_iter().enumerate() {
            let instance = people
                .create_person_instance(&crate::people::types::CreatePersonInstance {
                    folder_path: folder.clone(),
                    asset_path: image.clone(),
                    source_revision: "10:20".into(),
                    face_box: Some([0.1 + index as f64 * 0.4, 0.1, 0.2, 0.2]),
                    body_box: None,
                    request_id: format!("instance-{index}"),
                })
                .unwrap();
            let review = people
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
        let input = crate::people::types::SetPersonTagLink {
            folder_path: folder.clone(),
            subject_id: first.id.clone(),
            tag_id: Some(tag.id),
            enabled: true,
            expected_revision: 0,
            request_id: "bind-tag".into(),
        };
        let link = people.set_person_tag_link(&input).unwrap();
        assert_eq!(link, people.set_person_tag_link(&input).unwrap());
        assert_eq!(
            tags.asset_tag_source_kinds(&image, tag.id).unwrap(),
            ["person"]
        );
        assert_eq!(
            tags.asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            1
        );
        people
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
            tags.asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            1
        );
        let override_input = crate::people::types::SetPersonTagOverride {
            folder_path: folder.clone(),
            subject_id: input.subject_id.clone(),
            asset_path: image.clone(),
            suppressed: true,
            expected_revision: 0,
            request_id: "suppress-photo".into(),
        };
        let suppressed = people.set_person_tag_override(&override_input).unwrap();
        assert_eq!(
            suppressed,
            people.set_person_tag_override(&override_input).unwrap()
        );
        assert_eq!(
            tags.asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            0
        );
        people
            .set_person_tag_override(&crate::people::types::SetPersonTagOverride {
                suppressed: false,
                expected_revision: suppressed.revision,
                request_id: "restore-photo".into(),
                ..override_input
            })
            .unwrap();
        assert_eq!(
            tags.asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            1
        );
        tags.set_asset_tag(std::slice::from_ref(&image), tag.id, true)
            .unwrap();
        assert_eq!(
            tags.asset_tag_source_kinds(&image, tag.id).unwrap(),
            ["manual", "person"]
        );
        people
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
            tags.asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            1
        );
        tags.set_asset_tag(std::slice::from_ref(&image), tag.id, false)
            .unwrap();
        assert_eq!(
            tags.asset_tag_assignments(std::slice::from_ref(&image))
                .unwrap()[0]
                .assigned_count,
            0
        );
        tags.delete_custom_tag(tag.id).unwrap();
        let broken = people
            .get_person_tag_link(StdPath::new(&input.folder_path), &input.subject_id)
            .unwrap()
            .unwrap();
        assert_eq!(broken.tag_id, None);
        assert!(!broken.enabled);
    }

    /// A rename inside one folder is still the same photo, so the identity
    /// stays and only the path moves. A move out of the folder is a different
    /// asset: the person claim is dropped and the instance has to be reviewed.
    #[test]
    fn same_folder_rename_moves_person_facts_with_tag_sources() {
        use crate::people::People;
        use crate::people::types::{
            CreatePersonInstance, PersonReviewDecision, SetPersonReview, SetPersonTagLink,
        };

        let (tags, store) = in_memory_with_store();
        let people = People::new(Arc::clone(&store));
        let folder = PathBuf::from("/photos");
        let source = folder.join("before.jpg");
        let destination = folder.join("after.jpg");
        let tag = tags.create_custom_tag(None, "Alex").unwrap();
        let person = people.create_folder_person(&folder, "new-person").unwrap();
        let instance = people
            .create_person_instance(&CreatePersonInstance {
                folder_path: folder.clone(),
                asset_path: source.clone(),
                source_revision: "10:20".into(),
                face_box: Some([0.1, 0.1, 0.2, 0.2]),
                body_box: None,
                request_id: "new-face".into(),
            })
            .unwrap();
        {
            let connection = store.write();
            crate::people::repo::manual::insert_historical_person(
                &connection,
                "history",
                "Alex",
                &source.to_string_lossy(),
                "10:20",
            )
            .unwrap();
            crate::people::repo::manual::upsert_historical_link(&connection, &person.id, "history")
                .unwrap();
        }
        people
            .set_person_review(&SetPersonReview {
                folder_path: folder.clone(),
                instance_id: instance.id.clone(),
                subject_id: person.id.clone(),
                decision: PersonReviewDecision::Belongs,
                expected_revision: 0,
                request_id: "belongs".into(),
            })
            .unwrap();
        people
            .set_person_tag_link(&SetPersonTagLink {
                folder_path: folder.clone(),
                subject_id: person.id,
                tag_id: Some(tag.id),
                enabled: true,
                expected_revision: 0,
                request_id: "bind".into(),
            })
            .unwrap();
        relocate_state(&tags, &source, &destination).unwrap();
        assert!(
            people
                .list_person_instances(&folder, &source)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            people.list_person_instances(&folder, &destination).unwrap()[0].id,
            instance.id
        );
        assert_eq!(
            tags.asset_tag_source_kinds(&destination, tag.id).unwrap(),
            ["person"]
        );
        assert!(
            tags.asset_tag_source_kinds(&source, tag.id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            people.list_historical_people().unwrap()[0].reference_asset_path,
            destination
        );
        let elsewhere = PathBuf::from("/other/after.jpg");
        relocate_state(&tags, &destination, &elsewhere).unwrap();
        assert!(
            tags.asset_tag_source_kinds(&elsewhere, tag.id)
                .unwrap()
                .is_empty()
        );
        assert!(people.list_person_instances(&folder, &destination).unwrap()[0].needs_review);
    }
}
