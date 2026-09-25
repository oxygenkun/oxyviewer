//! Person identity, the review decisions over it, and the history of both.
//!
//! Everything here was named, confirmed, or rejected by the user, so it is
//! never dropped as a side effect of cache maintenance. The tables and their
//! `user` class are declared in `oxy_store::schema::user`, the derived
//! detection and feature caches that feed this module in
//! `oxy_store::schema::cache`, and every statement against them is a function
//! in `oxy_store::repo`.
//!
//! What stays here is what an identity *is*: that a review decision carries the
//! revision it was made against, that renaming a person is an event with a
//! request id, and what unlinking a historical person does to the links that
//! reference it.
//!
//! It also holds the bridge to the tag vocabulary — the one tag the identity
//! pushes onto its photos, and the per-photo exceptions to it. That bridge is
//! person policy: it decides what a link *means*, and it changes before the
//! tag side does. It reaches the tags through repository statements, so
//! `oxy-people` and `oxy-tags` stay independent.

use crate::{People, PeopleError};
use oxy_domain::{
    AssetSummary, ConfirmFolderPerson, CreatePersonInstance, FolderPerson, HistoricalPerson,
    LinkHistoricalPerson, PersonFilter, PersonFilterState, PersonInstance, PersonReview,
    PersonReviewDecision, PersonTagLink, PersonTagOverride, ResetFolderPerson, SetPersonReview,
    SetPersonTagLink, SetPersonTagOverride, UnlinkHistoricalPerson, UpdatePersonInstance,
};
use oxy_store::{StoreError, repo};
use std::{collections::HashMap, path::Path};

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn valid_box(value: Option<[f64; 4]>) -> bool {
    value.is_none_or(|[x, y, width, height]| {
        [x, y, width, height].iter().all(|part| part.is_finite())
            && x >= 0.0
            && y >= 0.0
            && width > 0.0
            && height > 0.0
            && x + width <= 1.0
            && y + height <= 1.0
    })
}

fn valid_source_identity(value: Option<&str>) -> bool {
    value
        .is_none_or(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

impl People {
    pub fn list_historical_people(&self) -> Result<Vec<HistoricalPerson>, PeopleError> {
        Ok(repo::people::list_historical_people(&self.store.read())?)
    }

    pub fn get_historical_link(
        &self,
        folder_path: &Path,
        subject_id: &str,
    ) -> Result<Option<String>, PeopleError> {
        Ok(repo::people::historical_link(
            &self.store.read(),
            &path_text(folder_path),
            subject_id,
        )?)
    }

    pub fn link_historical_person(
        &self,
        input: &LinkHistoricalPerson,
    ) -> Result<HistoricalPerson, PeopleError> {
        if input.request_id.is_empty() {
            return Err(PeopleError::InvalidPersonInstance);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        let history_id = if let Some((operation, id)) =
            repo::people::request_result(&transaction, &input.request_id)?
        {
            let replay_subject =
                repo::people::history_event_subject(&transaction, &input.request_id)?;
            if operation != "linkHistory"
                || replay_subject.as_deref() != Some(input.subject_id.as_str())
                || input
                    .historical_person_id
                    .as_ref()
                    .is_some_and(|expected| expected != &id)
            {
                return Err(PeopleError::PersonConflict);
            }
            id
        } else {
            let source = repo::people::link_source(
                &transaction,
                &input.subject_id,
                &path_text(&input.folder_path),
                input.expected_revision,
            )?;
            let Some((name, asset, revision)) = source else {
                return Err(PeopleError::PersonConflict);
            };
            let previous =
                repo::people::historical_link_of_subject(&transaction, &input.subject_id)?;
            if previous.is_some() && input.historical_person_id.is_none() {
                return Err(PeopleError::PersonConflict);
            }
            let id = if let Some(id) = &input.historical_person_id {
                if !repo::people::historical_person_exists(&transaction, id)? {
                    return Err(PeopleError::MissingPersonRecord);
                }
                id.clone()
            } else {
                let id = repo::people::new_id(&transaction)?;
                repo::people::insert_historical_person(&transaction, &id, &name, &asset, &revision)?;
                id
            };
            if previous.as_deref() == Some(id.as_str()) {
                return Err(PeopleError::PersonConflict);
            }
            repo::people::upsert_historical_link(&transaction, &input.subject_id, &id)?;
            repo::people::bump_subject_revision(&transaction, &input.subject_id)?;
            repo::people::insert_history_event(
                &transaction,
                &input.subject_id,
                &id,
                "link",
                &input.request_id,
            )?;
            repo::people::record_request_result(
                &transaction,
                &input.request_id,
                "linkHistory",
                &id,
            )?;
            repo::cross::reconcile_person_sources_for_subject(&transaction, &input.subject_id)?;
            id
        };
        let result = repo::people::historical_person(&transaction, &history_id)?
            .ok_or(PeopleError::MissingPersonRecord)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(result)
    }

    pub fn unlink_historical_person(
        &self,
        input: &UnlinkHistoricalPerson,
    ) -> Result<(), PeopleError> {
        if input.request_id.is_empty() {
            return Err(PeopleError::InvalidPersonInstance);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        if let Some((operation, subject)) =
            repo::people::request_result(&transaction, &input.request_id)?
        {
            if operation != "unlinkHistory" || subject != input.subject_id {
                return Err(PeopleError::PersonConflict);
            }
        } else {
            let history_id = repo::people::historical_link_at_revision(
                &transaction,
                &path_text(&input.folder_path),
                &input.subject_id,
                input.expected_revision,
            )?
            .ok_or(PeopleError::PersonConflict)?;
            repo::people::delete_historical_link(&transaction, &input.subject_id)?;
            repo::people::bump_subject_revision(&transaction, &input.subject_id)?;
            repo::people::insert_history_event(
                &transaction,
                &input.subject_id,
                &history_id,
                "unlink",
                &input.request_id,
            )?;
            repo::people::record_request_result(
                &transaction,
                &input.request_id,
                "unlinkHistory",
                &input.subject_id,
            )?;
            repo::cross::reconcile_person_sources_for_subject(&transaction, &input.subject_id)?;
        }
        transaction.commit().map_err(StoreError::from)?;
        Ok(())
    }

    /// The tag an identity pushes onto the photos it belongs to, if any.
    ///
    /// The link is the person side's statement; the effect lands as a `person`
    /// source on the tag side, which is why it is reconciled rather than
    /// written here. Reading the tag vocabulary to reject a link to a tag that
    /// does not exist goes through the repository, so this crate never names
    /// `oxy_tags` — the direction between the two crates stays one-way.
    pub fn get_person_tag_link(
        &self,
        folder_path: &Path,
        subject_id: &str,
    ) -> Result<Option<PersonTagLink>, PeopleError> {
        Ok(repo::people::person_tag_link(
            &self.store.read(),
            folder_path,
            subject_id,
        )?)
    }

    pub fn set_person_tag_link(
        &self,
        input: &SetPersonTagLink,
    ) -> Result<PersonTagLink, PeopleError> {
        if input.request_id.is_empty() || (input.enabled && input.tag_id.is_none()) {
            return Err(PeopleError::PersonConflict);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        let historical_id =
            repo::people::historical_person_of_subject(&transaction, &input.folder_path, &input.subject_id)?
                .ok_or(PeopleError::MissingPersonRecord)?;
        if let Some((operation, id)) = repo::people::request_result(&transaction, &input.request_id)? {
            if operation != "setPersonTagLink" || id != historical_id {
                return Err(PeopleError::PersonConflict);
            }
        } else {
            let revision = repo::people::person_tag_link_revision(&transaction, &historical_id)?;
            if revision.unwrap_or(0) != input.expected_revision {
                return Err(PeopleError::PersonConflict);
            }
            if let Some(tag_id) = input.tag_id
                && !repo::tags::tag_exists(&transaction, tag_id)?
            {
                return Err(PeopleError::MissingTagParent);
            }
            repo::people::upsert_person_tag_link(&transaction, &historical_id, input.tag_id, input.enabled)?;
            for subject in repo::people::subjects_of_historical_person(&transaction, &historical_id)? {
                repo::cross::reconcile_person_sources_for_subject(&transaction, &subject)?;
            }
            repo::people::record_request_result(
                &transaction,
                &input.request_id,
                "setPersonTagLink",
                &historical_id,
            )?;
        }
        let result = repo::people::person_tag_link_of(&transaction, &historical_id)?
            .ok_or(PeopleError::MissingPersonRecord)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(result)
    }

    pub fn get_person_tag_override(
        &self,
        folder_path: &Path,
        subject_id: &str,
        asset_path: &Path,
    ) -> Result<Option<PersonTagOverride>, PeopleError> {
        Ok(repo::people::person_tag_override(
            &self.store.read(),
            folder_path,
            subject_id,
            asset_path,
        )?)
    }

    /// Suppresses or restores the identity's tag for one photo.
    ///
    /// A whole folder can belong to a person while one picture in it does not,
    /// so the exception is recorded per asset and the tag side is reconciled
    /// for that asset alone.
    pub fn set_person_tag_override(
        &self,
        input: &SetPersonTagOverride,
    ) -> Result<PersonTagOverride, PeopleError> {
        if input.request_id.is_empty()
            || input.asset_path.parent() != Some(input.folder_path.as_path())
        {
            return Err(PeopleError::PersonConflict);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        let historical_id =
            repo::people::historical_person_of_subject(&transaction, &input.folder_path, &input.subject_id)?
                .ok_or(PeopleError::MissingPersonRecord)?;
        let request_entity = format!("{}:{}", historical_id, input.asset_path.display());
        let asset_path = input.asset_path.to_string_lossy();
        if let Some((operation, entity)) = repo::people::request_result(&transaction, &input.request_id)? {
            if operation != "setPersonTagOverride" || entity != request_entity {
                return Err(PeopleError::PersonConflict);
            }
        } else {
            let revision =
                repo::people::person_tag_override_revision(&transaction, &historical_id, &asset_path)?;
            if revision.unwrap_or(0) != input.expected_revision {
                return Err(PeopleError::PersonConflict);
            }
            repo::people::upsert_person_tag_override(
                &transaction,
                &historical_id,
                &asset_path,
                input.suppressed,
            )?;
            for subject in repo::people::subjects_of_historical_person(&transaction, &historical_id)? {
                repo::cross::reconcile_person_source_for_asset(&transaction, &subject, &asset_path)?;
            }
            repo::people::record_request_result(
                &transaction,
                &input.request_id,
                "setPersonTagOverride",
                &request_entity,
            )?;
        }
        let result = repo::people::person_tag_override_of(&transaction, &historical_id, &asset_path)?
            .ok_or(PeopleError::MissingPersonRecord)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(result)
    }

    /// Intersect the entire directory snapshot before sorting/paging, never a loaded UI page.
    pub fn filter_assets_by_person(
        &self,
        folder: &Path,
        assets: &[AssetSummary],
        filter: &PersonFilter,
    ) -> Result<Vec<AssetSummary>, PeopleError> {
        let records = repo::people::instance_records(
            &self.store.read(),
            &path_text(folder),
            filter.subject_id.as_deref(),
        )?;
        let mut by_path = HashMap::<String, Vec<_>>::new();
        for record in records {
            by_path.entry(record.asset_path).or_default().push((
                record.source_revision,
                record.needs_review,
                record.decision,
            ));
        }
        Ok(assets
            .iter()
            .filter(|asset| {
                let source = format!("{}:{}", asset.size_bytes, asset.modified_at_ms);
                let records = by_path.get(&path_text(&asset.path));
                if filter.state == PersonFilterState::Unassigned {
                    return records.is_none_or(|rows| {
                        !rows.iter().any(|(version, needs, decision)| {
                            version == &source && !needs && decision.is_some()
                        })
                    });
                }
                records.is_some_and(|rows| {
                    rows.iter().any(|(version, needs, decision)| {
                        let stale = version != &source || *needs;
                        if filter.state == PersonFilterState::NeedsReview {
                            return stale;
                        }
                        if stale {
                            return false;
                        }
                        match filter.state {
                            PersonFilterState::All => decision
                                .as_deref()
                                .is_some_and(|value| value != "doesNotBelong"),
                            PersonFilterState::Pending => decision.as_deref() == Some("pending"),
                            PersonFilterState::Belongs => decision.as_deref() == Some("belongs"),
                            PersonFilterState::DoesNotBelong => {
                                decision.as_deref() == Some("doesNotBelong")
                            }
                            PersonFilterState::Deferred => decision.as_deref() == Some("deferred"),
                            _ => false,
                        }
                    })
                })
            })
            .cloned()
            .collect())
    }

    pub fn update_person_instance(
        &self,
        input: &UpdatePersonInstance,
    ) -> Result<PersonInstance, PeopleError> {
        self.update_person_instance_with_source_identity(input, None)
    }

    pub fn update_person_instance_with_source_identity(
        &self,
        input: &UpdatePersonInstance,
        source_identity_revision: Option<&str>,
    ) -> Result<PersonInstance, PeopleError> {
        if input.request_id.is_empty()
            || input.source_revision.is_empty()
            || !valid_source_identity(source_identity_revision)
            || !valid_box(input.face_box)
            || !valid_box(input.body_box)
            || (input.face_box.is_none() && input.body_box.is_none())
        {
            return Err(PeopleError::InvalidPersonInstance);
        }
        let mut connection = self.store.write();
        let tx = connection.transaction().map_err(StoreError::from)?;
        if let Some((operation, entity)) =
            repo::people::request_result(&tx, &input.request_id)?
        {
            if operation != "updateInstance" || entity != input.instance_id {
                return Err(PeopleError::PersonConflict);
            }
            let stored = repo::people::source_identity_revision(&tx, &input.instance_id)?;
            if stored.as_deref() != source_identity_revision {
                return Err(PeopleError::PersonConflict);
            }
        } else {
            let previous = repo::people::instance(
                &tx,
                &input.instance_id,
                &path_text(&input.folder_path),
            )?
            .ok_or(PeopleError::MissingPersonRecord)?;
            if previous.revision != input.expected_revision {
                return Err(PeopleError::PersonConflict);
            }
            repo::people::insert_instance_event(
                &tx,
                &input.instance_id,
                &serde_json::to_string(&previous)?,
                &input.request_id,
            )?;
            repo::people::update_instance_geometry(
                &tx,
                &input.instance_id,
                input
                    .face_box
                    .map(|value| serde_json::to_string(&value))
                    .transpose()?
                    .as_deref(),
                input
                    .body_box
                    .map(|value| serde_json::to_string(&value))
                    .transpose()?
                    .as_deref(),
                &input.source_revision,
                source_identity_revision,
            )?;
            // A changed region is a new human claim: preserve old decisions in the audit,
            // and explicitly require review for every subject using this instance.
            repo::people::carry_review_decisions_into_events(
                &tx,
                &input.instance_id,
                &input.request_id,
            )?;
            repo::people::require_review_after_change(&tx, &input.instance_id)?;
            repo::people::bump_referencing_subjects(&tx, &input.instance_id)?;
            repo::people::delete_references_of_instance(&tx, &input.instance_id)?;
            repo::people::record_request_result(
                &tx,
                &input.request_id,
                "updateInstance",
                &input.instance_id,
            )?;
            for subject in repo::people::subjects_of_instance(&tx, &input.instance_id)? {
                repo::cross::reconcile_person_source_for_asset(
                    &tx,
                    &subject,
                    &path_text(&previous.asset_path),
                )?;
            }
        }
        let result =
            repo::people::instance(&tx, &input.instance_id, &path_text(&input.folder_path))?
                .ok_or(PeopleError::MissingPersonRecord)?;
        tx.commit().map_err(StoreError::from)?;
        Ok(result)
    }

    pub fn reset_folder_person(&self, input: &ResetFolderPerson) -> Result<(), PeopleError> {
        if input.request_id.is_empty() {
            return Err(PeopleError::InvalidPersonInstance);
        }
        let mut connection = self.store.write();
        let tx = connection.transaction().map_err(StoreError::from)?;
        if let Some((operation, entity)) = repo::people::request_result(&tx, &input.request_id)? {
            if operation != "resetPerson" || entity != input.subject_id {
                return Err(PeopleError::PersonConflict);
            }
        } else {
            let name = input.display_name.trim();
            let changed = repo::people::reset_identity(
                &tx,
                &input.subject_id,
                &path_text(&input.folder_path),
                name,
                input.expected_revision,
            )?;
            if changed != 1 {
                return Err(PeopleError::PersonConflict);
            }
            repo::people::carry_links_into_events(&tx, &input.subject_id, &input.request_id)?;
            repo::people::delete_historical_link(&tx, &input.subject_id)?;
            repo::people::delete_references_of_subject(&tx, &input.subject_id)?;
            repo::people::insert_identity_event(
                &tx,
                &input.subject_id,
                "reset",
                name,
                input.expected_revision + 1,
                &input.request_id,
            )?;
            repo::people::record_request_result(
                &tx,
                &input.request_id,
                "resetPerson",
                &input.subject_id,
            )?;
            repo::cross::reconcile_person_sources_for_subject(&tx, &input.subject_id)?;
        }
        tx.commit().map_err(StoreError::from)?;
        Ok(())
    }

    pub fn get_person_instance(
        &self,
        folder_path: &Path,
        id: &str,
    ) -> Result<PersonInstance, PeopleError> {
        repo::people::instance(&self.store.read(), id, &path_text(folder_path))?
            .ok_or(PeopleError::MissingPersonRecord)
    }

    pub fn confirm_folder_person(
        &self,
        input: &ConfirmFolderPerson,
    ) -> Result<FolderPerson, PeopleError> {
        let name = input.display_name.trim();
        if name.is_empty() || input.request_id.is_empty() {
            return Err(PeopleError::InvalidPersonInstance);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        if let Some((operation, entity)) =
            repo::people::request_result(&transaction, &input.request_id)?
        {
            if operation != "confirmPerson" || entity != input.subject_id {
                return Err(PeopleError::PersonConflict);
            }
        } else {
            let source_revision = repo::people::reference_source_revision(
                &transaction,
                &input.reference_instance_id,
                &path_text(&input.folder_path),
                &input.subject_id,
            )?
            .ok_or(PeopleError::MissingPersonRecord)?;
            let changed = repo::people::confirm_identity(
                &transaction,
                &input.subject_id,
                &path_text(&input.folder_path),
                name,
                input.expected_revision,
            )?;
            if changed != 1 {
                return Err(PeopleError::PersonConflict);
            }
            repo::people::delete_references_of_subject(&transaction, &input.subject_id)?;
            repo::people::replace_reference(
                &transaction,
                &input.subject_id,
                &input.reference_instance_id,
                &source_revision,
            )?;
            repo::people::insert_identity_event(
                &transaction,
                &input.subject_id,
                "confirm",
                name,
                input.expected_revision + 1,
                &input.request_id,
            )?;
            repo::people::record_request_result(
                &transaction,
                &input.request_id,
                "confirmPerson",
                &input.subject_id,
            )?;
        }
        let result = repo::people::folder_person(
            &transaction,
            &input.subject_id,
            &path_text(&input.folder_path),
        )?
        .ok_or(PeopleError::MissingPersonRecord)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(result)
    }

    pub fn list_folder_people(
        &self,
        folder_path: &Path,
    ) -> Result<Vec<FolderPerson>, PeopleError> {
        Ok(repo::people::list_folder_people(
            &self.store.read(),
            &path_text(folder_path),
        )?)
    }

    pub fn create_folder_person(
        &self,
        folder_path: &Path,
        request_id: &str,
    ) -> Result<FolderPerson, PeopleError> {
        if request_id.is_empty() {
            return Err(PeopleError::InvalidPersonInstance);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        let id = if let Some((operation, id)) =
            repo::people::request_result(&transaction, request_id)?
        {
            if operation != "createPerson" {
                return Err(PeopleError::PersonConflict);
            }
            id
        } else {
            let id = repo::people::new_id(&transaction)?;
            repo::people::insert_folder_person(&transaction, &id, &path_text(folder_path))?;
            repo::people::record_request_result(&transaction, request_id, "createPerson", &id)?;
            id
        };
        let result = repo::people::folder_person(&transaction, &id, &path_text(folder_path))?
            .ok_or(PeopleError::MissingPersonRecord)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(result)
    }

    pub fn create_person_instance(
        &self,
        input: &CreatePersonInstance,
    ) -> Result<PersonInstance, PeopleError> {
        self.create_person_instance_with_source_identity(input, None)
    }

    pub fn create_person_instance_with_source_identity(
        &self,
        input: &CreatePersonInstance,
        source_identity_revision: Option<&str>,
    ) -> Result<PersonInstance, PeopleError> {
        if input.source_revision.is_empty()
            || input.request_id.is_empty()
            || !valid_source_identity(source_identity_revision)
            || (input.face_box.is_none() && input.body_box.is_none())
            || !valid_box(input.face_box)
            || !valid_box(input.body_box)
        {
            return Err(PeopleError::InvalidPersonInstance);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        let id = if let Some((operation, id)) =
            repo::people::request_result(&transaction, &input.request_id)?
        {
            if operation != "createInstance" {
                return Err(PeopleError::PersonConflict);
            }
            let stored = repo::people::source_identity_revision(&transaction, &id)?;
            if stored.as_deref() != source_identity_revision {
                return Err(PeopleError::PersonConflict);
            }
            id
        } else {
            let id = repo::people::new_id(&transaction)?;
            repo::people::insert_instance(
                &transaction,
                &repo::people::NewInstance {
                    id: &id,
                    folder_path: &path_text(&input.folder_path),
                    asset_path: &path_text(&input.asset_path),
                    source_revision: &input.source_revision,
                    source_identity_revision,
                    face_box_json: input
                        .face_box
                        .map(|value| serde_json::to_string(&value))
                        .transpose()?
                        .as_deref(),
                    body_box_json: input
                        .body_box
                        .map(|value| serde_json::to_string(&value))
                        .transpose()?
                        .as_deref(),
                },
            )?;
            repo::people::record_request_result(
                &transaction,
                &input.request_id,
                "createInstance",
                &id,
            )?;
            id
        };
        let result = repo::people::instance(&transaction, &id, &path_text(&input.folder_path))?
            .ok_or(PeopleError::MissingPersonRecord)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(result)
    }

    pub fn list_person_instances(
        &self,
        folder_path: &Path,
        asset_path: &Path,
    ) -> Result<Vec<PersonInstance>, PeopleError> {
        Ok(repo::people::list_instances(
            &self.store.read(),
            &path_text(folder_path),
            &path_text(asset_path),
        )?)
    }

    pub fn list_manual_person_anchors(
        &self,
        folder_path: &Path,
        asset_path: &Path,
    ) -> Result<Vec<oxy_domain::ManualPersonAnchor>, PeopleError> {
        Ok(repo::people::list_anchors(
            &self.store.read(),
            &path_text(folder_path),
            &path_text(asset_path),
        )?)
    }

    pub fn set_person_review(&self, input: &SetPersonReview) -> Result<PersonReview, PeopleError> {
        if input.request_id.is_empty() {
            return Err(PeopleError::InvalidPersonInstance);
        }
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        let key = format!("{}:{}", input.instance_id, input.subject_id);
        if let Some((operation, entity)) =
            repo::people::request_result(&transaction, &input.request_id)?
        {
            if operation != "setReview" || entity != key {
                return Err(PeopleError::PersonConflict);
            }
        } else {
            let present = repo::people::review_allowed(
                &transaction,
                &input.instance_id,
                &input.subject_id,
                &path_text(&input.folder_path),
            )?;
            if !present {
                return Err(PeopleError::MissingPersonRecord);
            }
            let current = repo::people::review_revision(
                &transaction,
                &input.instance_id,
                &input.subject_id,
            )?;
            if current.unwrap_or(0) != input.expected_revision {
                return Err(PeopleError::PersonConflict);
            }
            repo::people::upsert_review_decision(
                &transaction,
                &input.instance_id,
                &input.subject_id,
                input.decision,
                input.expected_revision + 1,
            )?;
            if input.decision != PersonReviewDecision::Belongs {
                let removed = repo::people::delete_reference(
                    &transaction,
                    &input.subject_id,
                    &input.instance_id,
                )?;
                if removed > 0 {
                    repo::people::bump_subject_revision(&transaction, &input.subject_id)?;
                }
            }
            repo::people::insert_review_event(
                &transaction,
                &input.instance_id,
                &input.subject_id,
                input.decision,
                input.expected_revision + 1,
                &input.request_id,
            )?;
            repo::people::record_request_result(&transaction, &input.request_id, "setReview", &key)?;
            let asset_path =
                repo::people::instance_asset_path(&transaction, &input.instance_id)?;
            repo::cross::reconcile_person_source_for_asset(
                &transaction,
                &input.subject_id,
                &asset_path,
            )?;
        }
        let Some((instance, decision, revision)) =
            repo::people::review(&transaction, &input.instance_id, &input.subject_id)?
        else {
            return Err(PeopleError::MissingPersonRecord);
        };
        transaction.commit().map_err(StoreError::from)?;
        Ok(PersonReview {
            instance,
            subject_id: input.subject_id.clone(),
            decision,
            revision,
        })
    }

    pub fn list_person_reviews(
        &self,
        folder_path: &Path,
        subject_id: &str,
    ) -> Result<Vec<PersonReview>, PeopleError> {
        Ok(repo::people::list_reviews(
            &self.store.read(),
            &path_text(folder_path),
            subject_id,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use std::path::PathBuf;

    #[test]
    fn manual_reviews_are_scoped_idempotent_and_survive_reopen() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let folder = temporary.path().join("photos");
        let other_folder = temporary.path().join("other");
        let image = folder.join("group.jpg");
        let library = testing::open(&database);
        let first = library.create_folder_person(&folder, "create-one").unwrap();
        let second = library.create_folder_person(&folder, "create-two").unwrap();
        assert_eq!(
            first,
            library.create_folder_person(&folder, "create-one").unwrap()
        );
        assert!(
            library
                .list_folder_people(&other_folder)
                .unwrap()
                .is_empty()
        );

        let instance = library
            .create_person_instance(&CreatePersonInstance {
                folder_path: folder.clone(),
                asset_path: image.clone(),
                source_revision: "10:20".into(),
                face_box: Some([0.1, 0.2, 0.2, 0.3]),
                body_box: None,
                request_id: "face-one".into(),
            })
            .unwrap();
        let other_instance = library
            .create_person_instance(&CreatePersonInstance {
                folder_path: folder.clone(),
                asset_path: image,
                source_revision: "10:20".into(),
                face_box: Some([0.6, 0.2, 0.2, 0.3]),
                body_box: None,
                request_id: "face-two".into(),
            })
            .unwrap();
        assert_ne!(instance.id, other_instance.id);
        assert!(
            library
                .list_person_reviews(&folder, &first.id)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            library.confirm_folder_person(&ConfirmFolderPerson {
                folder_path: folder.clone(),
                subject_id: first.id.clone(),
                reference_instance_id: instance.id.clone(),
                display_name: "Alex".into(),
                expected_revision: first.revision,
                request_id: "too-early".into(),
            }),
            Err(PeopleError::MissingPersonRecord)
        ));
        let request = SetPersonReview {
            folder_path: folder.clone(),
            instance_id: instance.id,
            subject_id: first.id.clone(),
            decision: PersonReviewDecision::Belongs,
            expected_revision: 0,
            request_id: "review-one".into(),
        };
        let review = library.set_person_review(&request).unwrap();
        assert_eq!(review.revision, 1);
        assert_eq!(review, library.set_person_review(&request).unwrap());
        assert!(matches!(
            library.set_person_review(&SetPersonReview {
                request_id: "stale".into(),
                decision: PersonReviewDecision::DoesNotBelong,
                ..request
            }),
            Err(PeopleError::PersonConflict)
        ));
        assert!(
            library
                .list_person_reviews(&folder, &second.id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            library
                .list_person_reviews(&folder, &first.id)
                .unwrap()
                .len(),
            1
        );

        let confirmed = library
            .confirm_folder_person(&ConfirmFolderPerson {
                folder_path: folder.clone(),
                subject_id: first.id.clone(),
                reference_instance_id: review.instance.id.clone(),
                display_name: "Alex".into(),
                expected_revision: first.revision,
                request_id: "confirm-one".into(),
            })
            .unwrap();
        assert!(confirmed.identity_confirmed);
        let link_request = LinkHistoricalPerson {
            folder_path: folder.clone(),
            subject_id: first.id.clone(),
            historical_person_id: None,
            expected_revision: confirmed.revision,
            request_id: "history-one".into(),
        };
        let history = library.link_historical_person(&link_request).unwrap();
        assert_eq!(history.display_name, "Alex");
        assert_eq!(history.reference_source_revision, "10:20");
        assert_eq!(
            history,
            library.link_historical_person(&link_request).unwrap()
        );
        assert_eq!(
            library.get_historical_link(&folder, &first.id).unwrap(),
            Some(history.id.clone())
        );
        assert!(matches!(
            library.link_historical_person(&LinkHistoricalPerson {
                request_id: "history-stale".into(),
                ..link_request
            }),
            Err(PeopleError::PersonConflict)
        ));
        drop(library);

        let reopened = testing::open(&database);
        assert_eq!(
            reopened.list_person_reviews(&folder, &first.id).unwrap(),
            vec![review]
        );
        assert_eq!(
            reopened
                .list_folder_people(&folder)
                .unwrap()
                .into_iter()
                .find(|person| person.id == first.id),
            Some(FolderPerson {
                revision: confirmed.revision + 1,
                ..confirmed.clone()
            }),
        );
        assert_eq!(reopened.list_historical_people().unwrap(), vec![history]);
        assert_eq!(
            reopened
                .get_historical_link(&other_folder, &first.id)
                .unwrap(),
            None
        );
        let unlink = UnlinkHistoricalPerson {
            folder_path: folder.clone(),
            subject_id: first.id.clone(),
            expected_revision: confirmed.revision + 1,
            request_id: "unlink-history".into(),
        };
        reopened.unlink_historical_person(&unlink).unwrap();
        reopened.unlink_historical_person(&unlink).unwrap();
        assert_eq!(
            reopened.get_historical_link(&folder, &first.id).unwrap(),
            None
        );
        reopened
            .link_historical_person(&LinkHistoricalPerson {
                folder_path: folder.clone(),
                subject_id: first.id.clone(),
                historical_person_id: Some(
                    reopened.list_historical_people().unwrap()[0].id.clone(),
                ),
                expected_revision: confirmed.revision + 2,
                request_id: "relink-history".into(),
            })
            .unwrap();
        reopened
            .reset_folder_person(&ResetFolderPerson {
                folder_path: folder.clone(),
                subject_id: first.id.clone(),
                display_name: "Alex".into(),
                expected_revision: confirmed.revision + 3,
                request_id: "reset-history".into(),
            })
            .unwrap();
        assert_eq!(
            reopened.get_historical_link(&folder, &first.id).unwrap(),
            None
        );
    }

    #[test]
    fn filters_whole_snapshot_and_preserves_manual_history_after_corrections() {
        let library = testing::in_memory();
        let folder = PathBuf::from("/photos");
        let subject = library.create_folder_person(&folder, "person").unwrap();
        let assets = (0..301)
            .map(|index| AssetSummary {
                id: index.to_string(),
                path: folder.join(format!("{index:03}.jpg")),
                name: format!("{index:03}.jpg"),
                extension: "jpg".into(),
                kind: oxy_domain::AssetKind::Jpeg,
                size_bytes: 10,
                modified_at_ms: 20,
                has_sidecar: false,
                rating: Some(5),
                color_label: None,
                pick_label: None,
            })
            .collect::<Vec<_>>();
        let mut instances = Vec::new();
        for (index, asset_index) in [0, 0, 300].iter().enumerate() {
            let instance = library
                .create_person_instance(&CreatePersonInstance {
                    folder_path: folder.clone(),
                    asset_path: assets[*asset_index].path.clone(),
                    source_revision: "10:20".into(),
                    face_box: Some([0.1, 0.1, 0.2, 0.2]),
                    body_box: None,
                    request_id: format!("instance-{index}"),
                })
                .unwrap();
            library
                .set_person_review(&SetPersonReview {
                    folder_path: folder.clone(),
                    instance_id: instance.id.clone(),
                    subject_id: subject.id.clone(),
                    decision: PersonReviewDecision::Pending,
                    expected_revision: 0,
                    request_id: format!("review-{index}"),
                })
                .unwrap();
            instances.push(instance);
        }
        let filter = PersonFilter {
            subject_id: Some(subject.id.clone()),
            state: PersonFilterState::Pending,
        };
        let filtered = library
            .filter_assets_by_person(&folder, &assets, &filter)
            .unwrap();
        assert_eq!(filtered.len(), 2); // Two instances in one photo must not duplicate the photo.
        let page = oxy_fs::page_assets(
            &filtered,
            &oxy_domain::AssetQuery {
                page_size: Some(1),
                ..Default::default()
            },
            1,
        );
        assert_eq!(page.total, 2);
        assert_eq!(page.items[0].id, "300"); // Candidate beyond the ordinary first 250.
        let decide = |index: usize, decision, revision, request: &str| {
            library
                .set_person_review(&SetPersonReview {
                    folder_path: folder.clone(),
                    instance_id: instances[index].id.clone(),
                    subject_id: subject.id.clone(),
                    decision,
                    expected_revision: revision,
                    request_id: request.into(),
                })
                .unwrap()
        };
        decide(0, PersonReviewDecision::Belongs, 1, "belongs");
        assert_eq!(
            library
                .filter_assets_by_person(&folder, &assets, &filter)
                .unwrap()
                .len(),
            2
        ); // Other face still pending.
        let confirmed = library
            .confirm_folder_person(&ConfirmFolderPerson {
                folder_path: folder.clone(),
                subject_id: subject.id.clone(),
                reference_instance_id: instances[0].id.clone(),
                display_name: "Alex".into(),
                expected_revision: 1,
                request_id: "confirm".into(),
            })
            .unwrap();
        assert_eq!(
            confirmed.reference_instance_id,
            Some(instances[0].id.clone())
        );
        decide(
            0,
            PersonReviewDecision::DoesNotBelong,
            2,
            "reject-reference",
        );
        let person = library.list_folder_people(&folder).unwrap().remove(0);
        assert!(person.reference_instance_id.is_none());
        assert!(person.identity_confirmed); // Identity is separate from its reference.
        decide(1, PersonReviewDecision::Deferred, 1, "defer");
        assert_eq!(
            library
                .filter_assets_by_person(&folder, &assets, &filter)
                .unwrap()
                .len(),
            1
        );
        let correction = UpdatePersonInstance {
            folder_path: folder.clone(),
            instance_id: instances[0].id.clone(),
            source_revision: "10:20".into(),
            face_box: Some([0.2, 0.2, 0.3, 0.3]),
            body_box: Some([0.1, 0.1, 0.5, 0.8]),
            expected_revision: 1,
            request_id: "correct".into(),
        };
        let updated = library.update_person_instance(&correction).unwrap();
        assert_eq!(
            updated,
            library.update_person_instance(&correction).unwrap()
        );
        assert!(matches!(
            library.update_person_instance(&UpdatePersonInstance {
                request_id: "stale-correct".into(),
                ..correction
            }),
            Err(PeopleError::PersonConflict)
        ));
        assert_eq!(
            library
                .list_person_reviews(&folder, &subject.id)
                .unwrap()
                .iter()
                .find(|r| r.instance.id == instances[0].id)
                .unwrap()
                .decision,
            PersonReviewDecision::DoesNotBelong
        ); // Never silently revive explicit negatives.
        let mut changed = assets;
        changed[300].modified_at_ms = 21;
        assert!(
            library
                .filter_assets_by_person(&folder, &changed, &filter)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            library
                .filter_assets_by_person(
                    &folder,
                    &changed,
                    &PersonFilter {
                        state: PersonFilterState::NeedsReview,
                        ..filter
                    }
                )
                .unwrap()
                .len(),
            1
        );
        library
            .reset_folder_person(&ResetFolderPerson {
                folder_path: folder.clone(),
                subject_id: subject.id.clone(),
                display_name: "Temporary".into(),
                expected_revision: person.revision,
                request_id: "reset".into(),
            })
            .unwrap();
        assert!(!library.list_folder_people(&folder).unwrap()[0].identity_confirmed);
        assert_eq!(
            library
                .list_person_reviews(&folder, &subject.id)
                .unwrap()
                .len(),
            3
        );
        let connection = library.store.read();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM person_instance_events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn rejects_invalid_boxes_and_cross_folder_subjects() {
        let library = testing::in_memory();
        let folder = PathBuf::from("/one");
        let subject = library.create_folder_person(&folder, "subject").unwrap();
        let invalid = CreatePersonInstance {
            folder_path: folder.clone(),
            asset_path: folder.join("a.jpg"),
            source_revision: "1:2".into(),
            face_box: Some([0.9, 0.0, 0.2, 0.3]),
            body_box: None,
            request_id: "bad".into(),
        };
        assert!(matches!(
            library.create_person_instance(&invalid),
            Err(PeopleError::InvalidPersonInstance)
        ));
        let instance = library
            .create_person_instance(&CreatePersonInstance {
                face_box: Some([0.0, 0.0, 0.2, 0.3]),
                request_id: "good".into(),
                ..invalid
            })
            .unwrap();
        assert!(matches!(
            library.set_person_review(&SetPersonReview {
                folder_path: PathBuf::from("/other"),
                instance_id: instance.id,
                subject_id: subject.id,
                decision: PersonReviewDecision::Belongs,
                expected_revision: 0,
                request_id: "cross-folder".into(),
            }),
            Err(PeopleError::MissingPersonRecord)
        ));
    }

    #[test]
    fn full_source_identity_is_persisted_only_when_observed() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let library = testing::open(&database);
        let folder = PathBuf::from("/photos");
        let legacy = CreatePersonInstance {
            folder_path: folder.clone(),
            asset_path: folder.join("legacy.jpg"),
            source_revision: "10:20".into(),
            face_box: Some([0.1, 0.1, 0.2, 0.2]),
            body_box: None,
            request_id: "legacy".into(),
        };
        let old = library.create_person_instance(&legacy).unwrap();
        assert_eq!(
            library
                .list_manual_person_anchors(&folder, &legacy.asset_path)
                .unwrap()[0]
                .source_identity_revision,
            None
        );
        let revision = "a".repeat(64);
        let fresh = CreatePersonInstance {
            asset_path: folder.join("fresh.jpg"),
            request_id: "fresh".into(),
            ..legacy.clone()
        };
        let created = library
            .create_person_instance_with_source_identity(&fresh, Some(&revision))
            .unwrap();
        assert_eq!(
            library
                .create_person_instance_with_source_identity(&fresh, Some(&revision))
                .unwrap(),
            created
        );
        assert!(matches!(
            library.create_person_instance_with_source_identity(&fresh, Some(&"b".repeat(64))),
            Err(PeopleError::PersonConflict)
        ));
        let update = UpdatePersonInstance {
            folder_path: folder.clone(),
            instance_id: old.id,
            source_revision: "10:20".into(),
            face_box: Some([0.2, 0.2, 0.2, 0.2]),
            body_box: None,
            expected_revision: 1,
            request_id: "rebind".into(),
        };
        library
            .update_person_instance_with_source_identity(&update, Some(&revision))
            .unwrap();
        drop(library);
        let reopened = testing::open(&database);
        for path in [&legacy.asset_path, &fresh.asset_path] {
            let anchors = reopened.list_manual_person_anchors(&folder, path).unwrap();
            assert_eq!(
                anchors[0].source_identity_revision.as_deref(),
                Some(revision.as_str())
            );
        }
    }

    #[test]
    fn older_manual_table_migrates_without_assigning_unproven_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let connection = oxy_store::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE person_manual_instances (
               id TEXT PRIMARY KEY,folder_path TEXT NOT NULL,asset_path TEXT NOT NULL,
               source_revision TEXT NOT NULL,face_box TEXT,body_box TEXT,
               needs_review INTEGER NOT NULL DEFAULT 0,revision INTEGER NOT NULL DEFAULT 1,
               created_at INTEGER NOT NULL DEFAULT (unixepoch()),
               updated_at INTEGER NOT NULL DEFAULT (unixepoch()));
             INSERT INTO person_manual_instances(id,folder_path,asset_path,source_revision,face_box)
               VALUES ('old','/photos','/photos/a.jpg','10:20','[0.1,0.1,0.2,0.2]');",
            )
            .unwrap();
        drop(connection);
        let library = testing::open(&database);
        let anchors = library
            .list_manual_person_anchors(Path::new("/photos"), Path::new("/photos/a.jpg"))
            .unwrap();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].instance_id, "old");
        assert_eq!(anchors[0].source_identity_revision, None);
    }
}
