//! Explicit user action: copy suggestions to pending reviews in one transaction.
use super::*;
use crate::people::alignment::{AlignmentPolicy, DetectedAnchor, ManualAnchor, align_instances};
use crate::people::types::{AdoptPersonCluster, AdoptPersonClusterResult, PersonReviewDecision};

impl People {
    pub fn adopt_person_cluster(
        &self,
        input: &AdoptPersonCluster,
    ) -> Result<AdoptPersonClusterResult, PeopleError> {
        if input.request_id.trim().is_empty() {
            return Err(PeopleError::InvalidPersonInstance);
        }
        let folder = input.folder_path.canonicalize()?;
        let folder_text = folder.to_string_lossy();
        // Replays survive cache clearing: the explicit request is user-owned.
        if let Some((operation, json)) =
            crate::people::repo::manual::request_result(&self.store.read(), &input.request_id)?
        {
            if operation != "adoptCluster" {
                return Err(PeopleError::PersonConflict);
            }
            let (original, result): (serde_json::Value, AdoptPersonClusterResult) =
                serde_json::from_str(&json)?;
            if original != serde_json::to_value(input)? {
                return Err(PeopleError::PersonConflict);
            }
            return Ok(result);
        }
        let snapshot = self
            .person_clusters(&folder)?
            .filter(|s| s.snapshot_id == input.snapshot_id)
            .ok_or(PeopleError::PersonAnalysisConflict)?;
        let cluster = snapshot
            .clusters
            .iter()
            .find(|c| c.id == input.cluster_id)
            .ok_or(PeopleError::MissingPersonRecord)?;
        for member in &cluster.members {
            if crate::files::observe_file(&member.asset_path)?.revision_id()
                != member.source_revision
            {
                return Err(PeopleError::PersonAnalysisConflict);
            }
        }
        let mut connection = self.store.write();
        let tx = connection.transaction().map_err(StoreError::from)?;
        // The snapshot may have been invalidated while files were observed.
        let current: PersonClusterSnapshot = serde_json::from_str(
            &crate::people::repo::clusters::snapshot(&tx, &folder_text)?
                .ok_or(PeopleError::PersonAnalysisConflict)?,
        )?;
        if current.snapshot_id != snapshot.snapshot_id {
            return Err(PeopleError::PersonAnalysisConflict);
        }
        if crate::people::repo::manual::request_result(&tx, &input.request_id)?.is_some() {
            return Err(PeopleError::PersonConflict);
        }
        let subject = if let Some(id) = &input.subject_id {
            crate::people::repo::manual::folder_person(&tx, id, &folder_text)?
                .ok_or(PeopleError::MissingPersonRecord)?;
            id.clone()
        } else {
            let id = crate::people::repo::manual::new_id(&tx)?;
            crate::people::repo::manual::insert_folder_person(&tx, &id, &folder_text)?;
            if let Some(name) = input
                .display_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                crate::people::repo::manual::reset_identity(&tx, &id, &folder_text, name, 1)?;
                crate::people::repo::manual::insert_identity_event(
                    &tx,
                    &id,
                    "reset",
                    name,
                    2,
                    &format!("{}:name", input.request_id),
                )?;
            }
            id
        };
        let mut added = 0;
        let mut preserved = 0;
        let mut conflicted = 0;
        for member in &cluster.members {
            let asset = member.asset_path.to_string_lossy();
            let anchors = crate::people::repo::manual::list_anchors(&tx, &folder_text, &asset)?;
            let manual: Vec<_> = anchors
                .iter()
                .map(|a| ManualAnchor {
                    id: a.instance_id.clone(),
                    source_identity_revision: a.source_identity_revision.clone(),
                    face_box: a.face_box,
                    body_box: a.body_box,
                    needs_review: a.needs_review,
                })
                .collect();
            let detected: Vec<_> = snapshot
                .clusters
                .iter()
                .flat_map(|c| &c.members)
                .chain(&snapshot.ungrouped)
                .filter(|m| m.asset_path == member.asset_path)
                .map(|m| DetectedAnchor {
                    id: m.instance_id.clone(),
                    source_identity_revision: m.source_revision.clone(),
                    face_box: Some(m.face_box),
                    body_box: None,
                })
                .collect();
            let alignment = align_instances(
                &member.source_revision,
                &manual,
                &detected,
                AlignmentPolicy::default(),
            )
            .map_err(|_| PeopleError::InvalidPersonDetection)?;
            let instance = if let Some(matched) = alignment
                .matches
                .iter()
                .find(|m| m.detected_id == member.instance_id)
            {
                matched.manual_id.clone()
            } else if !alignment.conflicts.is_empty() {
                // A stale/edited/ambiguous anchor must not be bypassed by a fresh
                // instance that loses its explicit negative review.
                conflicted += 1;
                continue;
            } else {
                let id = crate::people::repo::manual::new_id(&tx)?;
                crate::people::repo::manual::insert_instance(
                    &tx,
                    &crate::people::repo::manual::NewInstance {
                        id: &id,
                        folder_path: &folder_text,
                        asset_path: &asset,
                        source_revision: &member.summary_revision,
                        source_identity_revision: Some(&member.source_revision),
                        face_box_json: Some(&serde_json::to_string(&member.face_box)?),
                        body_box_json: None,
                    },
                )?;
                id
            };
            if crate::people::repo::manual::review_revision(&tx, &instance, &subject)?.is_some() {
                preserved += 1;
                continue;
            }
            crate::people::repo::manual::upsert_review_decision(
                &tx,
                &instance,
                &subject,
                PersonReviewDecision::Pending,
                1,
            )?;
            crate::people::repo::manual::insert_review_event(
                &tx,
                &instance,
                &subject,
                PersonReviewDecision::Pending,
                1,
                &format!("{}:{instance}", input.request_id),
            )?;
            added += 1;
        }
        if added + preserved == 0 {
            return Err(PeopleError::Cluster(
                "本组的人工标注均需单独核对，请先处理冲突".into(),
            ));
        }
        crate::people::repo::manual::link_cluster(&tx, &folder_text, &cluster.id, &subject)?;
        let person = crate::people::repo::manual::folder_person(&tx, &subject, &folder_text)?
            .ok_or(PeopleError::MissingPersonRecord)?;
        let result = AdoptPersonClusterResult {
            person,
            added,
            preserved,
            conflicted,
        };
        crate::people::repo::manual::record_request_result(
            &tx,
            &input.request_id,
            "adoptCluster",
            &serde_json::to_string(&(input, &result))?,
        )?;
        tx.commit().map_err(StoreError::from)?;
        Ok(result)
    }
}
