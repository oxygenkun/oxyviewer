//! The people domain: durable persons, face decisions, and the undo journal.
//!
//! The rest of the face feature treats SQLite as a rebuildable cache: deleting
//! it may cost decode time and nothing else. Confirming "this is Alice" is
//! *not* rebuildable, so it cannot live only there. This module keeps that user
//! data in one small file, written through the crate's
//! [`DocumentStore`](crate::DocumentStore) so the envelope, the atomic
//! replacement, and the refusal to overwrite an unknown file are shared with
//! every other document instead of re-implemented here.
//!
//! The split of authority is deliberate and one-directional:
//!
//! ```text
//! PersonStore  ──authoritative──▶  SQLite projection
//!  people.json                      persons / face_decisions
//! ```
//!
//! A mutation is durable before it is visible: the store rewrites the file and
//! only then does the caller project it into the cache. A crash between the two
//! costs a projection refresh, never a confirmation.
//!
//! # Cost
//!
//! Every mutation rewrites the whole file. That is `O(persons + decisions)`
//! bytes per edit — a few hundred kilobytes for a large library, tens of
//! milliseconds with `fsync` — and it is bought knowingly: an append-only
//! journal would be faster but needs compaction and recovery logic that a
//! single self-describing file does not. If a library ever makes this visible,
//! the upgrade is a journal behind the same API, not a change to the caller.

use std::path::{Path, PathBuf};

use oxy_domain::{
    DecisionRecord, FaceDecision, FaceScoreSample, NormalizedRect, PersonId, PersonRecord,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::document::{DocumentStore, StoreError, StoreOrigin, UserDocument};

/// Current on-disk schema. A reader refuses a newer version rather than
/// guessing, so a future format cannot be silently truncated by an older build.
///
/// Version 2 adds the undo journal. A version 1 file still loads (the journal
/// defaults to empty), but an older build refuses a version 2 file instead of
/// rewriting it without the journal. Version 4 adds independent manual clarity marks.
pub const STORE_VERSION: u32 = 4;

/// How many person operations stay undoable. This is a journal for correcting a
/// misclick, not a history feature, so it is deliberately short.
pub const MAX_UNDO_OPERATIONS: usize = 64;

#[derive(Debug, Error)]
pub enum PeopleError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Filesystem(#[from] oxy_fs::FsError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("people store {path} has unsupported version {found}; this build supports {supported}")]
    UnsupportedVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    #[error("person name must not be empty")]
    InvalidName,
    #[error("person was not found: {0}")]
    MissingPerson(String),
    /// The store exists but this build must not write it. Only reachable when a
    /// caller asks for a write after loading refused the file.
    #[error("the people store refuses writes: {0}")]
    Refused(String),
}

impl From<StoreError> for PeopleError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Io(source) => Self::Io(source),
            StoreError::Filesystem(source) => Self::Filesystem(source),
            StoreError::Json(source) => Self::Json(source),
            // The document knows which store it is; this error names it too.
            StoreError::UnsupportedVersion {
                path,
                found,
                supported,
                ..
            } => Self::UnsupportedVersion {
                path,
                found,
                supported,
            },
            StoreError::Refused { reason, .. } => Self::Refused(reason),
        }
    }
}

/// The on-disk document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeopleDocument {
    pub version: u32,
    #[serde(default)]
    pub persons: Vec<PersonRecord>,
    #[serde(default)]
    pub decisions: Vec<DecisionRecord>,
    #[serde(default)]
    pub clarity_marks: Vec<oxy_domain::FaceClarityMark>,
    /// Append-only journal of reversible person operations, oldest first.
    #[serde(default)]
    pub operations: Vec<PersonOperation>,
    #[serde(default)]
    pub sidecars: std::collections::BTreeMap<PathBuf, oxy_domain::FaceSidecarState>,
}

impl UserDocument for PeopleDocument {
    const KIND: &'static str = "people";
    const CURRENT_VERSION: u32 = STORE_VERSION;

    fn version(&self) -> u32 {
        self.version
    }

    fn stamp(&mut self) {
        self.version = STORE_VERSION;
    }
}

/// One reversible operation on the person set.
///
/// Each variant stores the state needed to invert it exactly, so undo never has
/// to reconstruct a decision from the current (already changed) projection.
/// `Merge` stores the whole source person because undoing a merge must recreate
/// it with its original id and name, not merely re-point faces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "camelCase")]
pub enum PersonOperation {
    MergePersons {
        source: PersonRecord,
        target_person_id: PersonId,
        /// The decisions that were re-pointed, exactly as they were before the
        /// merge. Storing the records rather than their ids keeps undo exact
        /// even for a decision whose observation binding is already gone.
        moved: Vec<DecisionRecord>,
    },
    RemoveFaces {
        person_id: PersonId,
        /// The decisions that were removed, with their original bindings.
        removed: Vec<DecisionRecord>,
    },
    AssignFaces {
        person_id: PersonId,
        /// Observations the assignment confirmed, so undo removes exactly the
        /// rows it added rather than only the ones it replaced.
        assigned_observation_ids: Vec<String>,
        /// Decisions the assignment replaced, restored verbatim on undo.
        previous: Vec<DecisionRecord>,
    },
}

/// A short description of the newest undoable operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoableOperation {
    pub kind: String,
    /// Person the operation happened to (target for a merge).
    pub person_id: PersonId,
    /// How many faces the operation moved or removed.
    pub face_count: usize,
    /// Display name of the other person involved, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_person_name: Option<String>,
}

impl Default for PeopleDocument {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            persons: Vec::new(),
            decisions: Vec::new(),
            operations: Vec::new(),
            clarity_marks: Vec::new(),
            sidecars: Default::default(),
        }
    }
}

/// Durable user data for the people feature.
pub struct PersonStore {
    document: DocumentStore<PeopleDocument>,
}

impl std::fmt::Debug for PersonStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let path = self.document.path();
        self.document.with(|document| {
            formatter
                .debug_struct("PersonStore")
                .field("path", &path)
                .field("persons", &document.persons.len())
                .field("decisions", &document.decisions.len())
                .finish()
        })
    }
}

impl PersonStore {
    /// Loads the store, or creates an empty one when the file does not exist.
    ///
    /// A corrupt file, or one written by a newer build, is an error rather than
    /// an empty store: silently starting fresh would let the next projection
    /// overwrite real confirmations with nothing.
    pub fn load(path: impl Into<PathBuf>) -> Result<(Self, StoreOrigin), PeopleError> {
        let (document, origin) = DocumentStore::load(path)?;
        Ok((Self { document }, origin))
    }

    pub fn path(&self) -> &Path {
        self.document.path()
    }

    /// One durable batch. Identity decisions and measured clarity are untouched.
    /// A null override restores the analyzer judgement for those regions.
    pub fn set_clarity_marks(
        &self,
        regions: &[(PathBuf, NormalizedRect)],
        blurry: Option<bool>,
    ) -> Result<(), PeopleError> {
        if regions.iter().any(|(_, region)| !region.is_valid()) {
            return Err(PeopleError::Refused("invalid face quality region".into()));
        }
        self.document.update(|document| {
            for (path, region) in regions {
                document
                    .clarity_marks
                    .retain(|mark| mark.asset_path != *path || mark.region.iou(*region) < 0.5);
                if let Some(blurry) = blurry {
                    document.clarity_marks.push(oxy_domain::FaceClarityMark {
                        asset_path: path.clone(),
                        region: *region,
                        blurry,
                    });
                }
            }
        })?;
        Ok(())
    }

    /// Rebind by path and region after reanalysis, just like identity decisions.
    pub fn apply_clarity_marks(&self, items: &mut [oxy_domain::FaceReviewItem]) {
        self.document.with(|document| {
            let mut by_path: std::collections::HashMap<&Path, Vec<&oxy_domain::FaceClarityMark>> =
                std::collections::HashMap::new();
            for mark in &document.clarity_marks {
                by_path.entry(&mark.asset_path).or_default().push(mark);
            }
            for item in items {
                item.manual_blurry = by_path.get(item.asset_path.as_path()).and_then(|marks| {
                    marks
                        .iter()
                        .map(|mark| (mark, mark.region.iou(item.bbox)))
                        .filter(|(_, overlap)| *overlap >= 0.5)
                        .max_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(mark, _)| mark.blurry)
                });
            }
        });
    }

    pub fn sidecar_states(
        &self,
    ) -> std::collections::BTreeMap<PathBuf, oxy_domain::FaceSidecarState> {
        self.document.with(|document| document.sidecars.clone())
    }

    pub fn sidecar_state(&self, path: &Path) -> Option<oxy_domain::FaceSidecarState> {
        self.document
            .with(|document| document.sidecars.get(path).cloned())
    }

    pub fn merge_sidecar(
        &self,
        path: &Path,
        remote: oxy_domain::PortableFaceFacts,
    ) -> Result<bool, PeopleError> {
        if self.document.with(|document| {
            document
                .sidecars
                .get(path)
                .map_or(remote.facts.is_empty(), |state| {
                    state.base == remote && state.conflict.is_none() && state.last_error.is_none()
                })
        }) {
            return Ok(false);
        }
        Ok(self
            .document
            .update(|document| crate::face_sync::merge_remote(document, path, remote))?)
    }

    pub fn acknowledge_sidecar(
        &self,
        path: &Path,
        written: oxy_domain::PortableFaceFacts,
    ) -> Result<(), PeopleError> {
        self.document.update(|document| {
            let state = document.sidecars.entry(path.into()).or_default();
            state.base = written;
            state.last_error = None;
        })?;
        Ok(())
    }

    pub fn sidecar_error(&self, path: &Path, code: &str) -> Result<(), PeopleError> {
        if self.document.with(|document| {
            document
                .sidecars
                .get(path)
                .is_some_and(|state| state.last_error.as_deref() == Some(code))
        }) {
            return Ok(());
        }
        self.document.update(|document| {
            document.sidecars.entry(path.into()).or_default().last_error = Some(code.into());
        })?;
        Ok(())
    }

    pub fn resolve_sidecar(&self, path: &Path, use_remote: bool) -> Result<(), PeopleError> {
        self.document.update(|document| {
            let Some(state) = document.sidecars.get_mut(path) else {
                return;
            };
            let Some(remote) = state.conflict.take() else {
                return;
            };
            state.base = remote.clone();
            if use_remote {
                state.desired = remote.clone();
                crate::face_sync::import_facts(document, path, &remote);
                crate::face_sync::refresh_desired(document);
            }
        })?;
        Ok(())
    }

    pub fn sidecar_status(&self) -> Vec<oxy_domain::FaceSyncStatus> {
        self.document.with(|document| {
            document
                .sidecars
                .iter()
                .filter(|(_, state)| {
                    crate::face_sync::pending(state)
                        || state.conflict.is_some()
                        || state.last_error.is_some()
                })
                .map(|(path, state)| oxy_domain::FaceSyncStatus {
                    path: path.clone(),
                    pending: crate::face_sync::pending(state),
                    conflict: state.conflict.is_some(),
                    last_error: state.last_error.clone(),
                    local_facts: state.conflict.as_ref().map(|_| state.desired.clone()),
                    remote_facts: state.conflict.clone(),
                })
                .collect()
        })
    }

    /// One consistent snapshot for rebuilding a queryable projection.
    pub fn user_data(&self) -> (Vec<PersonRecord>, Vec<DecisionRecord>) {
        self.document
            .with(|document| (document.persons.clone(), document.decisions.clone()))
    }

    pub fn persons(&self) -> Vec<PersonRecord> {
        self.document.with(|document| document.persons.clone())
    }

    pub fn decisions(&self) -> Vec<DecisionRecord> {
        self.document.with(|document| document.decisions.clone())
    }

    pub fn person(&self, person_id: &str) -> Option<PersonRecord> {
        self.document.with(|document| {
            document
                .persons
                .iter()
                .find(|person| person.person_id == person_id)
                .cloned()
        })
    }

    /// Every user answer that carried a matcher proposal, as a score sample.
    ///
    /// "Not a face" is excluded: it is a statement about the detector, not about
    /// a match, so it says nothing about where the identity threshold belongs.
    pub fn score_samples(&self) -> Vec<FaceScoreSample> {
        self.document.with(|document| {
            document
                .decisions
                .iter()
                .filter_map(|record| {
                    let similarity = record.proposed_similarity?;
                    let accepted = match record.decision {
                        FaceDecision::ConfirmPerson { .. } => true,
                        FaceDecision::RejectPerson { .. } => false,
                        FaceDecision::NotFace => return None,
                    };
                    Some(FaceScoreSample {
                        similarity,
                        accepted,
                    })
                })
                .collect()
        })
    }

    pub fn decision_for(&self, observation_id: &str) -> Option<DecisionRecord> {
        self.document.with(|document| {
            document
                .decisions
                .iter()
                .find(|record| record.observation_id.as_deref() == Some(observation_id))
                .cloned()
        })
    }

    /// Adds or replaces a person. The caller supplies the id, so an import or
    /// an undo journal can reproduce it exactly.
    pub fn upsert_person(&self, person: PersonRecord) -> Result<(), PeopleError> {
        if person.display_name.trim().is_empty() {
            return Err(PeopleError::InvalidName);
        }
        self.mutate(|document| {
            match document
                .persons
                .iter_mut()
                .find(|existing| existing.person_id == person.person_id)
            {
                Some(existing) => *existing = person,
                None => document.persons.push(person),
            }
        })
    }

    pub fn rename_person(
        &self,
        person_id: &str,
        display_name: &str,
        now_ms: u64,
    ) -> Result<(), PeopleError> {
        let name = display_name.trim();
        if name.is_empty() {
            return Err(PeopleError::InvalidName);
        }
        let mut missing = false;
        self.mutate(|document| {
            match document
                .persons
                .iter_mut()
                .find(|person| person.person_id == person_id)
            {
                Some(person) => {
                    person.display_name = name.to_string();
                    person.updated_at_ms = now_ms;
                }
                None => missing = true,
            }
        })?;
        if missing {
            return Err(PeopleError::MissingPerson(person_id.to_string()));
        }
        Ok(())
    }

    pub fn link_tag(
        &self,
        person_id: &str,
        tag_id: Option<i64>,
        now_ms: u64,
    ) -> Result<(), PeopleError> {
        let mut missing = false;
        self.mutate(|document| {
            match document
                .persons
                .iter_mut()
                .find(|person| person.person_id == person_id)
            {
                Some(person) => {
                    person.linked_tag_id = tag_id;
                    person.updated_at_ms = now_ms;
                }
                None => missing = true,
            }
        })?;
        if missing {
            return Err(PeopleError::MissingPerson(person_id.to_string()));
        }
        Ok(())
    }

    /// Removes a person and every decision that named it.
    ///
    /// `NotFace` decisions survive: they describe a region, not a person, and
    /// deleting the person must not resurrect a face the user dismissed.
    /// Returns the number of removed decisions.
    pub fn remove_person(&self, person_id: &str) -> Result<usize, PeopleError> {
        let mut removed = 0usize;
        let mut missing = false;
        self.mutate(|document| {
            if !document
                .persons
                .iter()
                .any(|person| person.person_id == person_id)
            {
                missing = true;
                return;
            }
            let before = document.decisions.len();
            document
                .decisions
                .retain(|record| record.decision.person_id() != Some(person_id));
            removed = before - document.decisions.len();
            document
                .persons
                .retain(|person| person.person_id != person_id);
        })?;
        if missing {
            return Err(PeopleError::MissingPerson(person_id.to_string()));
        }
        Ok(removed)
    }

    /// Records the user's answer about one face region.
    ///
    /// The record is keyed by `observation_id`, so re-answering the same face
    /// replaces the previous answer for it. Records whose observation was
    /// replaced keep their region and their `None` binding.
    pub fn set_decision(&self, record: DecisionRecord) -> Result<(), PeopleError> {
        if let Some(person_id) = record.decision.person_id() {
            let known = self.document.with(|document| {
                document
                    .persons
                    .iter()
                    .any(|person| person.person_id == person_id)
            });
            if !known {
                return Err(PeopleError::MissingPerson(person_id.to_string()));
            }
        }
        if !record.region.is_valid() {
            return Err(PeopleError::InvalidName);
        }
        self.mutate(|document| {
            if let Some(observation_id) = record.observation_id.as_deref() {
                document
                    .decisions
                    .retain(|existing| existing.observation_id.as_deref() != Some(observation_id));
            }
            document.decisions.push(record);
        })
    }

    /// Removes the answer for one face, returning it to the review queue.
    pub fn clear_decision(&self, observation_id: &str) -> Result<bool, PeopleError> {
        let mut removed = false;
        self.mutate(|document| {
            let before = document.decisions.len();
            document
                .decisions
                .retain(|record| record.observation_id.as_deref() != Some(observation_id));
            removed = document.decisions.len() != before;
        })?;
        Ok(removed)
    }

    /// Replaces every decision binding for `asset_path` after a re-analysis.
    ///
    /// Durable decisions are matched to new observations by region overlap, the
    /// same rule the cache projection uses. A decision whose face is gone keeps
    /// its region and loses its binding.
    pub fn rebind_asset(
        &self,
        asset_path: &Path,
        observations: &[(String, NormalizedRect)],
        rebind_iou: f32,
    ) -> Result<usize, PeopleError> {
        let asset_path_string = asset_path.to_string_lossy().to_string();
        let mut rebound = 0usize;
        self.mutate(|document| {
            for record in document.decisions.iter_mut() {
                if record.asset_path.to_string_lossy() != asset_path_string {
                    continue;
                }
                let best = observations
                    .iter()
                    .map(|(id, region)| (id, region.iou(record.region)))
                    .filter(|(_, iou)| *iou >= rebind_iou)
                    .max_by(|left, right| {
                        left.1
                            .partial_cmp(&right.1)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(id, _)| id.clone());
                if best != record.observation_id {
                    record.observation_id = best;
                    rebound += 1;
                }
            }
        })?;
        Ok(rebound)
    }

    /// Folds `source_person_id` into `target_person_id`.
    ///
    /// Every decision that named the source now names the target, and the source
    /// person disappears. This is the "Alice + Alica" repair: the two names were
    /// always one person, and the faces must not be re-confirmed by hand.
    ///
    /// The whole source record is journalled, so [`PersonStore::undo`] restores
    /// the person with its original id and name.
    pub fn merge_persons(
        &self,
        source_person_id: &str,
        target_person_id: &str,
    ) -> Result<usize, PeopleError> {
        if source_person_id == target_person_id {
            return Ok(0);
        }
        let mut moved: Vec<DecisionRecord> = Vec::new();
        let mut source_record = None;
        let mut missing = None;
        self.mutate(|document| {
            source_record = document
                .persons
                .iter()
                .find(|person| person.person_id == source_person_id)
                .cloned();
            if source_record.is_none() {
                missing = Some(source_person_id.to_string());
                return;
            }
            if !document
                .persons
                .iter()
                .any(|person| person.person_id == target_person_id)
            {
                missing = Some(target_person_id.to_string());
                return;
            }
            for record in document.decisions.iter_mut() {
                if record.decision.person_id() != Some(source_person_id) {
                    continue;
                }
                let replacement = match &record.decision {
                    FaceDecision::ConfirmPerson { .. } => FaceDecision::ConfirmPerson {
                        person_id: target_person_id.to_string(),
                    },
                    FaceDecision::RejectPerson { .. } => FaceDecision::RejectPerson {
                        person_id: target_person_id.to_string(),
                    },
                    FaceDecision::NotFace => continue,
                };
                moved.push(record.clone());
                record.decision = replacement;
            }
            document
                .persons
                .retain(|person| person.person_id != source_person_id);
            let operation = PersonOperation::MergePersons {
                source: source_record.clone().expect("checked above"),
                target_person_id: target_person_id.to_string(),
                moved: moved.clone(),
            };
            push_operation(document, operation);
        })?;
        if let Some(person_id) = missing {
            return Err(PeopleError::MissingPerson(person_id));
        }
        Ok(moved.len())
    }

    /// Drops the user's confirmation of `observation_ids` for one person.
    ///
    /// The faces return to the unknown queue and can be named again or assigned
    /// elsewhere. "Not this person" and "not a face" decisions are untouched:
    /// they are statements about a region, not about this person.
    ///
    /// This is the "split the three wrong faces out of Alice" repair, and it
    /// leaves Alice's other photos exactly as they were.
    pub fn remove_faces_from_person(
        &self,
        person_id: &str,
        observation_ids: &[String],
    ) -> Result<usize, PeopleError> {
        let wanted: std::collections::HashSet<&str> =
            observation_ids.iter().map(String::as_str).collect();
        let mut removed_records = Vec::new();
        let mut missing = false;
        self.mutate(|document| {
            if !document
                .persons
                .iter()
                .any(|person| person.person_id == person_id)
            {
                missing = true;
                return;
            }
            let mut kept = Vec::with_capacity(document.decisions.len());
            for record in document.decisions.drain(..) {
                let matches = record
                    .observation_id
                    .as_deref()
                    .is_some_and(|id| wanted.contains(id))
                    && record.decision.person_id() == Some(person_id);
                if matches {
                    removed_records.push(record);
                } else {
                    kept.push(record);
                }
            }
            document.decisions = kept;
            if !removed_records.is_empty() {
                push_operation(
                    document,
                    PersonOperation::RemoveFaces {
                        person_id: person_id.to_string(),
                        removed: removed_records.clone(),
                    },
                );
            }
        })?;
        if missing {
            return Err(PeopleError::MissingPerson(person_id.to_string()));
        }
        Ok(removed_records.len())
    }

    /// Confirms a set of faces for one person, replacing any previous answer.
    pub fn assign_faces_to_person(
        &self,
        person_id: &str,
        observations: &[(String, String, std::path::PathBuf, NormalizedRect)],
        now_ms: u64,
    ) -> Result<usize, PeopleError> {
        let mut previous = Vec::new();
        let mut missing = false;
        self.mutate(|document| {
            if !document
                .persons
                .iter()
                .any(|person| person.person_id == person_id)
            {
                missing = true;
                return;
            }
            for (observation_id, asset_id, asset_path, region) in observations {
                if let Some(index) = document.decisions.iter().position(|record| {
                    record.observation_id.as_deref() == Some(observation_id.as_str())
                }) {
                    previous.push(document.decisions.remove(index));
                }
                document.decisions.push(DecisionRecord {
                    observation_id: Some(observation_id.clone()),
                    asset_id: asset_id.clone(),
                    asset_path: asset_path.clone(),
                    region: *region,
                    decision: FaceDecision::ConfirmPerson {
                        person_id: person_id.to_string(),
                    },
                    created_at_ms: now_ms,
                    // A manual assignment replaces whatever the matcher
                    // proposed, so there is no proposal score behind it.
                    proposed_similarity: None,
                });
            }
            if !observations.is_empty() {
                push_operation(
                    document,
                    PersonOperation::AssignFaces {
                        person_id: person_id.to_string(),
                        assigned_observation_ids: observations
                            .iter()
                            .map(|(observation_id, _, _, _)| observation_id.clone())
                            .collect(),
                        previous: previous.clone(),
                    },
                );
            }
        })?;
        if missing {
            return Err(PeopleError::MissingPerson(person_id.to_string()));
        }
        Ok(previous.len())
    }

    /// Description of the newest undoable operation, if any.
    pub fn undoable(&self) -> Option<UndoableOperation> {
        self.document.with(|document| {
            document
                .operations
                .last()
                .map(|operation| describe_operation(operation, document))
        })
    }

    /// Reverses the newest person operation. Returns what was undone.
    ///
    /// Undo is exact: each operation carries the state it replaced, so this
    /// never has to infer a previous decision from the current projection.
    pub fn undo(&self) -> Result<Option<UndoableOperation>, PeopleError> {
        let mut undone = None;
        self.mutate(|document| {
            let Some(operation) = document.operations.pop() else {
                return;
            };
            undone = Some(describe_operation(&operation, document));
            match operation {
                PersonOperation::MergePersons {
                    source,
                    target_person_id,
                    moved,
                } => {
                    document.persons.push(source);
                    for before in moved {
                        // Find the row this decision became, then restore it.
                        let Some(record) = document.decisions.iter_mut().find(|record| {
                            same_decision_slot(record, &before)
                                && record.decision.person_id() == Some(target_person_id.as_str())
                        }) else {
                            continue;
                        };
                        record.decision = before.decision;
                    }
                }
                PersonOperation::RemoveFaces { removed, .. } => {
                    document.decisions.extend(removed);
                }
                PersonOperation::AssignFaces {
                    person_id,
                    assigned_observation_ids,
                    previous,
                } => {
                    let assigned: std::collections::HashSet<&str> = assigned_observation_ids
                        .iter()
                        .map(String::as_str)
                        .collect();
                    document.decisions.retain(|record| {
                        let was_assigned = record
                            .observation_id
                            .as_deref()
                            .is_some_and(|id| assigned.contains(id))
                            && record.decision.person_id() == Some(person_id.as_str());
                        !was_assigned
                    });
                    document.decisions.extend(previous);
                }
            }
        })?;
        Ok(undone)
    }

    /// Re-points the decisions made on one asset to its new path.
    ///
    /// A rename or move must not lose a confirmation: the decision describes a
    /// region of those bytes, and the bytes are the same. Observation bindings
    /// are preserved, because the caller moves the cache rows with them.
    ///
    /// These file-driven transfers are deliberately *not* journalled: undoing a
    /// file operation is the file service's job, not the person journal's.
    pub fn move_decisions(
        &self,
        source: &Path,
        destination: &Path,
        destination_asset_id: &str,
    ) -> Result<usize, PeopleError> {
        let source_string = source.to_string_lossy().to_string();
        let mut moved = 0usize;
        self.mutate(|document| {
            if let Some(state) = document.sidecars.remove(source) {
                document.sidecars.insert(destination.into(), state);
            }
            for mark in &mut document.clarity_marks {
                if mark.asset_path == source {
                    mark.asset_path = destination.into();
                }
            }
            for record in document.decisions.iter_mut() {
                if record.asset_path.to_string_lossy() != source_string {
                    continue;
                }
                record.asset_path = destination.to_path_buf();
                record.asset_id = destination_asset_id.to_string();
                moved += 1;
            }
        })?;
        Ok(moved)
    }

    /// Duplicates the decisions made on one asset onto a copy of it.
    ///
    /// The copy has the same pixels but no observations yet, so its decisions
    /// start unbound and re-attach when the copy is analyzed.
    pub fn copy_decisions(
        &self,
        source: &Path,
        destination: &Path,
        destination_asset_id: &str,
    ) -> Result<usize, PeopleError> {
        let source_string = source.to_string_lossy().to_string();
        let mut copied = 0usize;
        self.mutate(|document| {
            // The file service copies the sidecar too; keep its fact IDs so
            // the first synchronization does not invent overlapping facts.
            if let Some(state) = document.sidecars.get(source).cloned() {
                document.sidecars.insert(destination.into(), state);
            }
            let marks: Vec<_> = document
                .clarity_marks
                .iter()
                .filter(|mark| mark.asset_path == source)
                .map(|mark| oxy_domain::FaceClarityMark {
                    asset_path: destination.into(),
                    ..mark.clone()
                })
                .collect();
            document.clarity_marks.extend(marks);
            let clones: Vec<DecisionRecord> = document
                .decisions
                .iter()
                .filter(|record| record.asset_path.to_string_lossy() == source_string)
                .map(|record| DecisionRecord {
                    observation_id: None,
                    asset_id: destination_asset_id.to_string(),
                    asset_path: destination.to_path_buf(),
                    region: record.region,
                    decision: record.decision.clone(),
                    created_at_ms: record.created_at_ms,
                    proposed_similarity: record.proposed_similarity,
                })
                .collect();
            copied = clones.len();
            document.decisions.extend(clones);
        })?;
        Ok(copied)
    }

    /// Drops the decisions for an asset that no longer exists.
    pub fn remove_decisions(&self, path: &Path) -> Result<usize, PeopleError> {
        let path_string = path.to_string_lossy().to_string();
        let mut removed = 0usize;
        self.mutate(|document| {
            // Intentional file deletion also removes its sidecar. Do not queue
            // a write forever to a photo that the user has just deleted.
            document.sidecars.remove(path);
            document
                .clarity_marks
                .retain(|mark| mark.asset_path != path);
            let before = document.decisions.len();
            document
                .decisions
                .retain(|record| record.asset_path.to_string_lossy() != path_string);
            removed = before - document.decisions.len();
        })?;
        Ok(removed)
    }

    /// Rewrites the durable file with the given data. Used once, when an
    /// existing cache is migrated into a store that did not exist yet.
    pub fn delete_all_annotations(&self) -> Result<(), PeopleError> {
        self.mutate(|document| {
            document.clarity_marks.clear();
            document.persons.clear();
            document.decisions.clear();
            document.operations.clear();
            for state in document.sidecars.values_mut() {
                if let Some(remote) = state.conflict.take() {
                    for fact in &remote.facts {
                        if !state.desired.facts.iter().any(|local| local.id == fact.id) {
                            state.desired.facts.push(fact.clone());
                        }
                    }
                    state.base = remote;
                }
            }
        })
    }

    pub fn replace_all(
        &self,
        persons: Vec<PersonRecord>,
        decisions: Vec<DecisionRecord>,
    ) -> Result<(), PeopleError> {
        self.mutate(|document| {
            document.persons = persons;
            document.decisions = decisions;
        })
    }

    /// Applies one change to the durable document.
    ///
    /// The store writes the new bytes before publishing them, so a mutation that
    /// cannot be persisted never becomes visible: without that order the caller
    /// would project a decision that is lost on the next start.
    fn mutate(&self, change: impl FnOnce(&mut PeopleDocument)) -> Result<(), PeopleError> {
        self.document.update(|document| {
            change(document);
            crate::face_sync::refresh_desired(document);
        })?;
        Ok(())
    }
}

/// True when two records describe the same user answer slot.
///
/// Observation id is the normal key. A decision whose observation was replaced
/// has none, so its asset plus region and kind stand in — enough to find the
/// row again without depending on a cache binding that no longer exists.
fn same_decision_slot(left: &DecisionRecord, right: &DecisionRecord) -> bool {
    match (&left.observation_id, &right.observation_id) {
        (Some(left_id), Some(right_id)) => left_id == right_id,
        (None, None) => {
            left.asset_path == right.asset_path
                && std::mem::discriminant(&left.decision) == std::mem::discriminant(&right.decision)
                && (left.region.x - right.region.x).abs() < 1e-6
                && (left.region.y - right.region.y).abs() < 1e-6
                && (left.region.width - right.region.width).abs() < 1e-6
                && (left.region.height - right.region.height).abs() < 1e-6
        }
        _ => false,
    }
}

fn push_operation(document: &mut PeopleDocument, operation: PersonOperation) {
    document.operations.push(operation);
    let excess = document
        .operations
        .len()
        .saturating_sub(MAX_UNDO_OPERATIONS);
    if excess > 0 {
        document.operations.drain(..excess);
    }
}

fn describe_operation(operation: &PersonOperation, document: &PeopleDocument) -> UndoableOperation {
    let name_of = |person_id: &str| {
        document
            .persons
            .iter()
            .find(|person| person.person_id == person_id)
            .map(|person| person.display_name.clone())
    };
    match operation {
        PersonOperation::MergePersons {
            source,
            target_person_id,
            moved,
        } => UndoableOperation {
            kind: "mergePersons".to_string(),
            person_id: target_person_id.clone(),
            face_count: moved.len(),
            other_person_name: Some(source.display_name.clone()),
        },
        PersonOperation::RemoveFaces { person_id, removed } => UndoableOperation {
            kind: "removeFaces".to_string(),
            person_id: person_id.clone(),
            face_count: removed.len(),
            other_person_name: name_of(person_id),
        },
        PersonOperation::AssignFaces {
            person_id,
            assigned_observation_ids,
            ..
        } => UndoableOperation {
            kind: "assignFaces".to_string(),
            person_id: person_id.clone(),
            face_count: assigned_observation_ids.len(),
            other_person_name: name_of(person_id),
        },
    }
}

/// Decisions whose person no longer exists are dropped while loading.
///
/// This is a defensive repair for a hand-edited or partially restored file; it
/// keeps the invariant that every person reference resolves.
pub fn prune_orphaned_decisions(document: &mut PeopleDocument) -> usize {
    let known: std::collections::HashSet<PersonId> = document
        .persons
        .iter()
        .map(|person| person.person_id.clone())
        .collect();
    let before = document.decisions.len();
    document.decisions.retain(|record| match &record.decision {
        FaceDecision::ConfirmPerson { person_id } | FaceDecision::RejectPerson { person_id } => {
            known.contains(person_id)
        }
        FaceDecision::NotFace => true,
    });
    before - document.decisions.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_domain::FaceDecision;

    fn person(id: &str, name: &str) -> PersonRecord {
        PersonRecord {
            tag_path: None,
            person_id: id.into(),
            display_name: name.into(),
            linked_tag_id: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn decision(observation: &str, asset: &str, decision: FaceDecision) -> DecisionRecord {
        DecisionRecord {
            observation_id: Some(observation.into()),
            asset_id: asset.into(),
            asset_path: PathBuf::from(asset),
            region: NormalizedRect::new(0.1, 0.1, 0.2, 0.2),
            decision,
            created_at_ms: 1,
            proposed_similarity: None,
        }
    }

    fn quality_item(path: &str) -> oxy_domain::FaceReviewItem {
        oxy_domain::FaceReviewItem {
            observation_id: "new-observation".into(),
            asset_id: "asset".into(),
            asset_path: path.into(),
            bbox: NormalizedRect::new(0.1, 0.1, 0.2, 0.2),
            detection_score: 0.9,
            face_pixels: 64,
            clarity: 0.8,
            manual_blurry: None,
            state: oxy_domain::FaceReviewState::Confirmed,
            candidate: None,
            cluster_id: None,
            confirmed_person_id: Some("p1".into()),
            confirmed_person_name: Some("Alice".into()),
        }
    }

    #[test]
    fn manual_quality_survives_reload_and_keeps_identity_and_scores() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("people.json");
        std::fs::write(&path, r#"{"version":3,"persons":[],"decisions":[]}"#).unwrap();
        let (store, _) = PersonStore::load(&path).unwrap();
        let mut rows = vec![quality_item("/photo.jpg")];
        let regions = vec![(rows[0].asset_path.clone(), rows[0].bbox)];
        store.set_clarity_marks(&regions, Some(true)).unwrap();
        let (store, _) = PersonStore::load(&path).unwrap();
        store.apply_clarity_marks(&mut rows);
        assert_eq!(rows[0].manual_blurry, Some(true));
        assert_eq!(rows[0].clarity, 0.8);
        assert_eq!(rows[0].confirmed_person_id.as_deref(), Some("p1"));
        store.set_clarity_marks(&regions, Some(false)).unwrap();
        store.apply_clarity_marks(&mut rows);
        assert_eq!(rows[0].manual_blurry, Some(false));
        store.set_clarity_marks(&regions, None).unwrap();
        store.apply_clarity_marks(&mut rows);
        assert_eq!(rows[0].manual_blurry, None);
        let document: PeopleDocument =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(document.version, 4);
    }

    #[test]
    fn manual_quality_follows_file_operations_but_not_other_faces() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        let row = quality_item("/source.jpg");
        store
            .set_clarity_marks(&[(row.asset_path.clone(), row.bbox)], Some(true))
            .unwrap();
        store
            .copy_decisions(Path::new("/source.jpg"), Path::new("/copy.jpg"), "copy")
            .unwrap();
        store
            .move_decisions(Path::new("/source.jpg"), Path::new("/moved.jpg"), "moved")
            .unwrap();
        let mut rows = vec![
            quality_item("/source.jpg"),
            quality_item("/copy.jpg"),
            quality_item("/moved.jpg"),
        ];
        store.apply_clarity_marks(&mut rows);
        assert_eq!(
            rows.iter().map(|row| row.manual_blurry).collect::<Vec<_>>(),
            [None, Some(true), Some(true)]
        );
        rows[2].bbox = NormalizedRect::new(0.7, 0.7, 0.1, 0.1);
        store.remove_decisions(Path::new("/copy.jpg")).unwrap();
        store.apply_clarity_marks(&mut rows);
        assert!(rows.iter().all(|row| row.manual_blurry.is_none()));
        store.delete_all_annotations().unwrap();
        assert!(
            store
                .document
                .with(|document| document.clarity_marks.is_empty())
        );
    }

    #[test]
    fn a_missing_file_starts_empty_and_reports_creation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("people.json");
        let (store, origin) = PersonStore::load(&path).unwrap();
        assert_eq!(origin, StoreOrigin::Created);
        assert!(store.persons().is_empty());
        assert!(store.decisions().is_empty());
        // Loading an empty store must not create the file.
        assert!(!path.exists());
    }

    #[test]
    fn mutations_are_durable_before_they_are_visible() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("people.json");
        {
            let (store, _) = PersonStore::load(&path).unwrap();
            store.upsert_person(person("p1", "Alice")).unwrap();
            store
                .set_decision(decision(
                    "obs-1",
                    "/photos/a.jpg",
                    FaceDecision::ConfirmPerson {
                        person_id: "p1".into(),
                    },
                ))
                .unwrap();
            assert!(path.is_file(), "the first mutation writes the file");
        }
        let (reloaded, origin) = PersonStore::load(&path).unwrap();
        assert_eq!(origin, StoreOrigin::Loaded);
        assert_eq!(reloaded.persons().len(), 1);
        assert_eq!(reloaded.persons()[0].display_name, "Alice");
        assert_eq!(reloaded.decisions().len(), 1);
        assert_eq!(
            reloaded.decisions()[0].decision,
            FaceDecision::ConfirmPerson {
                person_id: "p1".into()
            }
        );
    }

    #[test]
    fn re_answering_a_face_replaces_only_that_answer() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("p1", "Alice")).unwrap();
        store.upsert_person(person("p2", "Bob")).unwrap();
        store
            .set_decision(decision(
                "obs-1",
                "/photos/a.jpg",
                FaceDecision::ConfirmPerson {
                    person_id: "p1".into(),
                },
            ))
            .unwrap();
        store
            .set_decision(decision("obs-2", "/photos/a.jpg", FaceDecision::NotFace))
            .unwrap();
        store
            .set_decision(decision(
                "obs-1",
                "/photos/a.jpg",
                FaceDecision::RejectPerson {
                    person_id: "p2".into(),
                },
            ))
            .unwrap();

        assert_eq!(store.decisions().len(), 2);
        assert_eq!(
            store.decision_for("obs-1").unwrap().decision,
            FaceDecision::RejectPerson {
                person_id: "p2".into()
            }
        );
        assert_eq!(
            store.decision_for("obs-2").unwrap().decision,
            FaceDecision::NotFace
        );
    }

    #[test]
    fn deleting_a_person_keeps_not_face_but_removes_its_decisions() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("p1", "Alice")).unwrap();
        store
            .set_decision(decision(
                "obs-1",
                "/photos/a.jpg",
                FaceDecision::ConfirmPerson {
                    person_id: "p1".into(),
                },
            ))
            .unwrap();
        store
            .set_decision(decision("obs-2", "/photos/a.jpg", FaceDecision::NotFace))
            .unwrap();

        let removed = store.remove_person("p1").unwrap();
        assert_eq!(removed, 1);
        assert!(store.persons().is_empty());
        let remaining = store.decisions();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].decision, FaceDecision::NotFace);
        assert!(matches!(
            store.remove_person("p1"),
            Err(PeopleError::MissingPerson(_))
        ));
    }

    #[test]
    fn a_decision_requires_an_existing_person_and_valid_region() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        assert!(matches!(
            store.set_decision(decision(
                "obs-1",
                "/photos/a.jpg",
                FaceDecision::ConfirmPerson {
                    person_id: "ghost".into()
                }
            )),
            Err(PeopleError::MissingPerson(_))
        ));

        store.upsert_person(person("p1", "Alice")).unwrap();
        let mut broken = decision(
            "obs-1",
            "/photos/a.jpg",
            FaceDecision::ConfirmPerson {
                person_id: "p1".into(),
            },
        );
        broken.region = NormalizedRect::new(0.1, 0.1, 0.0, 0.2);
        assert!(store.set_decision(broken).is_err());
        assert!(store.decisions().is_empty());
        assert!(matches!(
            store.upsert_person(person("p2", "   ")),
            Err(PeopleError::InvalidName)
        ));
    }

    #[test]
    fn rebinding_moves_a_decision_onto_the_overlapping_new_observation() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("p1", "Alice")).unwrap();
        store
            .set_decision(decision(
                "old-obs",
                "/photos/a.jpg",
                FaceDecision::ConfirmPerson {
                    person_id: "p1".into(),
                },
            ))
            .unwrap();

        let observations = vec![
            (
                "new-obs".to_string(),
                NormalizedRect::new(0.105, 0.098, 0.205, 0.198),
            ),
            ("other".to_string(), NormalizedRect::new(0.7, 0.7, 0.1, 0.1)),
        ];
        assert_eq!(
            store
                .rebind_asset(Path::new("/photos/a.jpg"), &observations, 0.5)
                .unwrap(),
            1
        );
        assert_eq!(
            store.decision_for("new-obs").unwrap().decision,
            FaceDecision::ConfirmPerson {
                person_id: "p1".into()
            }
        );
        assert!(store.decision_for("old-obs").is_none());
    }

    #[test]
    fn a_decision_whose_face_is_gone_loses_its_binding_but_stays() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store
            .set_decision(decision("obs-1", "/photos/a.jpg", FaceDecision::NotFace))
            .unwrap();
        let rebound = store
            .rebind_asset(
                Path::new("/photos/a.jpg"),
                &[(
                    "unrelated".to_string(),
                    NormalizedRect::new(0.7, 0.7, 0.1, 0.1),
                )],
                0.5,
            )
            .unwrap();
        assert_eq!(rebound, 1);
        let records = store.decisions();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].observation_id, None);
        assert_eq!(records[0].decision, FaceDecision::NotFace);
    }

    #[test]
    fn a_newer_file_version_is_refused_instead_of_truncated() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("people.json");
        std::fs::write(&path, br#"{"version": 99, "persons": [], "decisions": []}"#).unwrap();
        assert!(matches!(
            PersonStore::load(&path),
            Err(PeopleError::UnsupportedVersion { found: 99, .. })
        ));
    }

    #[test]
    fn a_corrupt_file_is_an_error_rather_than_an_empty_store() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("people.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(matches!(
            PersonStore::load(&path),
            Err(PeopleError::Json(_))
        ));
    }

    #[test]
    fn orphaned_decisions_are_pruned_on_demand() {
        let mut document = PeopleDocument {
            version: STORE_VERSION,
            operations: Vec::new(),
            clarity_marks: Vec::new(),
            sidecars: Default::default(),
            persons: vec![person("p1", "Alice")],
            decisions: vec![
                decision(
                    "obs-1",
                    "/photos/a.jpg",
                    FaceDecision::ConfirmPerson {
                        person_id: "p1".into(),
                    },
                ),
                decision(
                    "obs-2",
                    "/photos/a.jpg",
                    FaceDecision::ConfirmPerson {
                        person_id: "ghost".into(),
                    },
                ),
                decision("obs-3", "/photos/a.jpg", FaceDecision::NotFace),
            ],
        };
        assert_eq!(prune_orphaned_decisions(&mut document), 1);
        assert_eq!(document.decisions.len(), 2);
    }

    #[test]
    fn replace_all_is_the_migration_entry_point() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("people.json");
        let (store, _) = PersonStore::load(&path).unwrap();
        store
            .replace_all(
                vec![person("p1", "Alice")],
                vec![decision("obs-1", "/photos/a.jpg", FaceDecision::NotFace)],
            )
            .unwrap();
        let (reloaded, origin) = PersonStore::load(&path).unwrap();
        assert_eq!(origin, StoreOrigin::Loaded);
        assert_eq!(reloaded.persons().len(), 1);
        assert_eq!(reloaded.decisions().len(), 1);
    }
}

#[cfg(test)]
mod operation_tests {
    use super::*;
    use oxy_domain::FaceDecision;

    fn person(id: &str, name: &str) -> PersonRecord {
        PersonRecord {
            tag_path: None,
            person_id: id.into(),
            display_name: name.into(),
            linked_tag_id: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn confirmed(observation: &str, person_id: &str) -> DecisionRecord {
        DecisionRecord {
            observation_id: Some(observation.into()),
            asset_id: "asset".into(),
            asset_path: std::path::PathBuf::from("/photos/a.jpg"),
            region: NormalizedRect::new(0.1, 0.1, 0.2, 0.2),
            decision: FaceDecision::ConfirmPerson {
                person_id: person_id.into(),
            },
            created_at_ms: 1,
            proposed_similarity: None,
        }
    }

    fn reopened(store: &PersonStore) -> PersonStore {
        PersonStore::load(store.path()).unwrap().0
    }

    #[test]
    fn merging_moves_every_decision_and_removes_the_source_person() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("alice", "Alice")).unwrap();
        store.upsert_person(person("alica", "Alica")).unwrap();
        store.set_decision(confirmed("obs-1", "alice")).unwrap();
        store.set_decision(confirmed("obs-2", "alica")).unwrap();
        store.set_decision(confirmed("obs-3", "alica")).unwrap();
        // A "not this person" statement follows the person it names.
        store
            .set_decision(DecisionRecord {
                observation_id: Some("obs-4".into()),
                asset_id: "asset".into(),
                asset_path: std::path::PathBuf::from("/photos/a.jpg"),
                region: NormalizedRect::new(0.4, 0.1, 0.2, 0.2),
                decision: FaceDecision::RejectPerson {
                    person_id: "alica".into(),
                },
                created_at_ms: 1,
                proposed_similarity: None,
            })
            .unwrap();

        let moved = store.merge_persons("alica", "alice").unwrap();
        assert_eq!(moved, 3, "two confirmations and one rejection");
        assert_eq!(store.persons().len(), 1);
        assert_eq!(store.persons()[0].person_id, "alice");
        assert!(
            store
                .decisions()
                .iter()
                .all(|record| record.decision.person_id() == Some("alice"))
        );
        assert_eq!(store.decisions().len(), 4, "no decision may be dropped");
    }

    #[test]
    fn undo_restores_a_merge_exactly() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("alice", "Alice")).unwrap();
        store.upsert_person(person("alica", "Alica")).unwrap();
        store.set_decision(confirmed("obs-1", "alice")).unwrap();
        store.set_decision(confirmed("obs-2", "alica")).unwrap();
        let before = store.decisions();

        store.merge_persons("alica", "alice").unwrap();
        let undoable = store.undoable().expect("a merge is undoable");
        assert_eq!(undoable.kind, "mergePersons");
        assert_eq!(undoable.face_count, 1);
        assert_eq!(undoable.other_person_name.as_deref(), Some("Alica"));

        store.undo().unwrap().expect("undone");
        assert_eq!(store.persons().len(), 2);
        assert!(store.person("alica").is_some(), "the id and name come back");
        assert_eq!(store.person("alica").unwrap().display_name, "Alica");
        assert_eq!(
            store.decisions(),
            before,
            "the journal restores exact state"
        );
        assert!(store.undoable().is_none(), "each operation undoes once");
    }

    #[test]
    fn removing_faces_leaves_the_rest_of_the_person_alone() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("alice", "Alice")).unwrap();
        for observation in ["obs-1", "obs-2", "obs-3", "obs-4"] {
            store.set_decision(confirmed(observation, "alice")).unwrap();
        }
        store
            .set_decision(DecisionRecord {
                observation_id: Some("obs-5".into()),
                asset_id: "asset".into(),
                asset_path: std::path::PathBuf::from("/photos/a.jpg"),
                region: NormalizedRect::new(0.5, 0.5, 0.1, 0.1),
                decision: FaceDecision::NotFace,
                created_at_ms: 1,
                proposed_similarity: None,
            })
            .unwrap();

        let removed = store
            .remove_faces_from_person("alice", &["obs-2".into(), "obs-3".into()])
            .unwrap();
        assert_eq!(removed, 2);
        let remaining: Vec<String> = store
            .decisions()
            .iter()
            .filter_map(|record| record.observation_id.clone())
            .collect();
        assert_eq!(remaining, vec!["obs-1", "obs-4", "obs-5"]);

        store.undo().unwrap();
        assert_eq!(store.decisions().len(), 5, "the removed photos return");
        assert!(store.undoable().is_none());
    }

    #[test]
    fn assigning_faces_replaces_previous_answers_and_can_be_undone() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("alice", "Alice")).unwrap();
        store.upsert_person(person("bob", "Bob")).unwrap();
        store.set_decision(confirmed("obs-1", "bob")).unwrap();
        store
            .set_decision(DecisionRecord {
                observation_id: Some("obs-2".into()),
                asset_id: "asset".into(),
                asset_path: std::path::PathBuf::from("/photos/a.jpg"),
                region: NormalizedRect::new(0.3, 0.1, 0.2, 0.2),
                decision: FaceDecision::NotFace,
                created_at_ms: 1,
                proposed_similarity: None,
            })
            .unwrap();

        store
            .assign_faces_to_person(
                "alice",
                &[
                    (
                        "obs-1".to_string(),
                        "asset".to_string(),
                        std::path::PathBuf::from("/photos/a.jpg"),
                        NormalizedRect::new(0.1, 0.1, 0.2, 0.2),
                    ),
                    (
                        "obs-2".to_string(),
                        "asset".to_string(),
                        std::path::PathBuf::from("/photos/a.jpg"),
                        NormalizedRect::new(0.3, 0.1, 0.2, 0.2),
                    ),
                ],
                2,
            )
            .unwrap();
        assert_eq!(
            store.decisions().len(),
            2,
            "obs-2 is replaced, not duplicated"
        );
        assert!(
            store
                .decisions()
                .iter()
                .all(|record| record.decision.person_id() == Some("alice"))
        );

        store.undo().unwrap();
        assert_eq!(
            store.decision_for("obs-1").unwrap().decision,
            FaceDecision::ConfirmPerson {
                person_id: "bob".into()
            }
        );
        assert_eq!(
            store.decision_for("obs-2").unwrap().decision,
            FaceDecision::NotFace,
            "an overwritten non-person decision comes back"
        );
    }

    #[test]
    fn the_journal_survives_a_reopen_and_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("people.json");
        let (store, _) = PersonStore::load(&path).unwrap();
        store.upsert_person(person("alice", "Alice")).unwrap();
        for index in 0..(MAX_UNDO_OPERATIONS + 10) {
            store
                .assign_faces_to_person(
                    "alice",
                    &[(
                        format!("obs-{index}"),
                        "asset".to_string(),
                        std::path::PathBuf::from("/photos/a.jpg"),
                        NormalizedRect::new(0.1, 0.1, 0.2, 0.2),
                    )],
                    1,
                )
                .unwrap();
        }
        let reopened = reopened(&store);
        assert!(reopened.undoable().is_some(), "the journal is durable");
        // Undo every stored operation, then confirm the journal is bounded.
        for _ in 0..MAX_UNDO_OPERATIONS {
            reopened.undo().unwrap();
        }
        assert!(reopened.undoable().is_none());
        // Only the newest operations were retained, so the oldest assignment
        // is still in place.
        assert_eq!(
            reopened.decisions().len(),
            10,
            "operations beyond the journal window are permanent"
        );
    }

    #[test]
    fn operations_on_a_missing_person_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("alice", "Alice")).unwrap();
        assert!(matches!(
            store.merge_persons("ghost", "alice"),
            Err(PeopleError::MissingPerson(_))
        ));
        assert!(matches!(
            store.merge_persons("alice", "ghost"),
            Err(PeopleError::MissingPerson(_))
        ));
        assert!(matches!(
            store.remove_faces_from_person("ghost", &[]),
            Err(PeopleError::MissingPerson(_))
        ));
        assert_eq!(store.merge_persons("alice", "alice").unwrap(), 0);
        assert!(store.undoable().is_none());
        assert_eq!(store.persons().len(), 1, "a refused merge changes nothing");
    }

    #[test]
    fn a_version_one_file_loads_with_an_empty_journal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("people.json");
        std::fs::write(
            &path,
            br#"{"version":1,"persons":[{"personId":"p1","displayName":"Alice",
                "createdAtMs":1,"updatedAtMs":1}],"decisions":[]}"#,
        )
        .unwrap();
        let (store, origin) = PersonStore::load(&path).unwrap();
        assert_eq!(origin, StoreOrigin::Loaded);
        assert_eq!(store.persons().len(), 1);
        assert!(store.undoable().is_none());
    }
}

#[cfg(test)]
mod file_move_tests {
    use super::*;
    use oxy_domain::FaceDecision;

    fn person(id: &str, name: &str) -> PersonRecord {
        PersonRecord {
            tag_path: None,
            person_id: id.into(),
            display_name: name.into(),
            linked_tag_id: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn confirmed(observation: &str, asset: &str, person_id: &str) -> DecisionRecord {
        DecisionRecord {
            observation_id: Some(observation.into()),
            asset_id: format!("id:{asset}"),
            asset_path: std::path::PathBuf::from(asset),
            region: NormalizedRect::new(0.1, 0.1, 0.2, 0.2),
            decision: FaceDecision::ConfirmPerson {
                person_id: person_id.into(),
            },
            created_at_ms: 1,
            proposed_similarity: None,
        }
    }

    #[test]
    fn a_rename_keeps_the_confirmation_on_the_same_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("alice", "Alice")).unwrap();
        store
            .set_decision(confirmed("obs-1", "/photos/IMG_1.jpg", "alice"))
            .unwrap();
        store
            .set_decision(confirmed("obs-2", "/photos/other.jpg", "alice"))
            .unwrap();

        let moved = store
            .move_decisions(
                std::path::Path::new("/photos/IMG_1.jpg"),
                std::path::Path::new("/photos/Tokyo-001.jpg"),
                "id:/photos/Tokyo-001.jpg",
            )
            .unwrap();
        assert_eq!(moved, 1, "only the renamed asset moves");
        let record = store.decision_for("obs-1").expect("still present");
        assert_eq!(
            record.asset_path,
            std::path::PathBuf::from("/photos/Tokyo-001.jpg")
        );
        assert_eq!(record.asset_id, "id:/photos/Tokyo-001.jpg");
        assert_eq!(
            record.observation_id.as_deref(),
            Some("obs-1"),
            "the cache binding stays valid because the caller moves it too"
        );
        assert!(
            store.decision_for("obs-2").is_some(),
            "another asset is untouched"
        );
    }

    #[test]
    fn a_copy_carries_the_confirmation_but_not_the_binding() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("alice", "Alice")).unwrap();
        store
            .set_decision(confirmed("obs-1", "/photos/a.jpg", "alice"))
            .unwrap();

        let copied = store
            .copy_decisions(
                std::path::Path::new("/photos/a.jpg"),
                std::path::Path::new("/backup/a.jpg"),
                "id:/backup/a.jpg",
            )
            .unwrap();
        assert_eq!(copied, 1);
        assert_eq!(store.decisions().len(), 2);
        // The original keeps its binding; the copy starts unbound.
        assert_eq!(
            store
                .decision_for("obs-1")
                .unwrap()
                .observation_id
                .as_deref(),
            Some("obs-1")
        );
        let clone = store
            .decisions()
            .into_iter()
            .find(|record| record.asset_path == Path::new("/backup/a.jpg"))
            .expect("the copy exists");
        assert_eq!(clone.observation_id, None);
        assert_eq!(clone.decision.person_id(), Some("alice"));
    }

    #[test]
    fn deleting_an_asset_drops_its_decisions_only() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("alice", "Alice")).unwrap();
        store
            .set_decision(confirmed("obs-1", "/photos/a.jpg", "alice"))
            .unwrap();
        store
            .set_decision(confirmed("obs-2", "/photos/b.jpg", "alice"))
            .unwrap();

        assert_eq!(
            store
                .remove_decisions(std::path::Path::new("/photos/a.jpg"))
                .unwrap(),
            1
        );
        assert_eq!(store.decisions().len(), 1);
        assert!(store.person("alice").is_some(), "the person survives");
        assert_eq!(
            store.decisions()[0].asset_path,
            std::path::PathBuf::from("/photos/b.jpg")
        );
    }

    #[test]
    fn only_matcher_answers_become_score_samples() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("alice", "Alice")).unwrap();
        store
            .set_decision(DecisionRecord {
                proposed_similarity: Some(0.62),
                ..confirmed("obs-1", "/photos/a.jpg", "alice")
            })
            .unwrap();
        store
            .set_decision(DecisionRecord {
                proposed_similarity: Some(0.40),
                ..confirmed("obs-2", "/photos/a.jpg", "alice")
            })
            .unwrap();
        // A named-from-scratch face has no proposal behind it.
        store
            .set_decision(confirmed("obs-3", "/photos/a.jpg", "alice"))
            .unwrap();
        // "Not a face" says nothing about the identity threshold.
        store
            .set_decision(DecisionRecord {
                proposed_similarity: Some(0.9),
                decision: FaceDecision::NotFace,
                ..confirmed("obs-4", "/photos/a.jpg", "alice")
            })
            .unwrap();

        let samples = store.score_samples();
        assert_eq!(samples.len(), 2, "only answered proposals are samples");
        assert_eq!(samples[0].similarity, 0.62);
        assert!(samples[0].accepted);
        assert_eq!(samples[1].similarity, 0.40);
        assert!(samples[1].accepted, "a confirmed proposal is an accept");
    }

    #[test]
    fn a_rejected_proposal_is_a_negative_sample() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        store.upsert_person(person("alice", "Alice")).unwrap();
        store
            .set_decision(DecisionRecord {
                proposed_similarity: Some(0.38),
                decision: FaceDecision::RejectPerson {
                    person_id: "alice".into(),
                },
                ..confirmed("obs-1", "/photos/a.jpg", "alice")
            })
            .unwrap();
        let samples = store.score_samples();
        assert_eq!(samples.len(), 1);
        assert!(!samples[0].accepted);
    }

    #[test]
    fn moving_an_asset_with_no_decisions_is_a_no_op() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = PersonStore::load(directory.path().join("people.json")).unwrap();
        assert_eq!(
            store
                .move_decisions(
                    std::path::Path::new("/photos/absent.jpg"),
                    std::path::Path::new("/photos/new.jpg"),
                    "id"
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .copy_decisions(
                    std::path::Path::new("/photos/absent.jpg"),
                    std::path::Path::new("/photos/new.jpg"),
                    "id"
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .remove_decisions(std::path::Path::new("/photos/absent.jpg"))
                .unwrap(),
            0
        );
    }
}
