//! One app-wide identity registry; folder groups are projections over tuples.
use crate::{People, PeopleError};
use oxy_domain::*;
use oxy_store::{
    Connection, StoreError,
    repo::{self, global_people as db},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

mod grouping;
mod mutations;
#[cfg(test)]
mod tests;

fn hash(value: &impl Serialize) -> Result<String, PeopleError> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn failure(message: &str) -> PeopleError {
    PeopleError::Cluster(message.into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Suggestions {
    version: u32,
    run_id: String,
    pipeline: String,
    groups: Vec<PersonTupleGroup>,
    no_face_count: usize,
    unavailable_count: usize,
    notice: Option<String>,
}

fn migrate(c: &Connection) -> Result<(), PeopleError> {
    // Retain all legacy facts/events. Only explicit historical links merge identities.
    for p in repo::people::list_historical_people(c)? {
        db::insert_person(c, &p.id, &p.display_name)?;
        db::migrate_tag(c, &p.id)?;
    }
    let legacy = db::legacy_subjects(c)?;
    for p in &legacy {
        let id = p
            .history_id
            .clone()
            .unwrap_or_else(|| format!("legacy:{}", p.id));
        db::insert_person(c, &id, &p.name)?;
        for old in repo::people::list_reviews(c, &p.folder, &p.id)? {
            let previous = db::reviews(c, Some(&p.folder))?
                .into_iter()
                .find(|r| r.instance_id == old.instance.id && r.person_id == id);
            let decision = match previous {
                Some(r) if r.decision != old.decision => {
                    if r.decision == PersonReviewDecision::DoesNotBelong
                        || old.decision == PersonReviewDecision::DoesNotBelong
                    {
                        PersonReviewDecision::DoesNotBelong
                    } else {
                        PersonReviewDecision::Deferred
                    }
                }
                _ => old.decision,
            };
            db::review(c, &old.instance.id, &id, decision)?;
            if decision != PersonReviewDecision::Belongs {
                db::remove_reference(c, &id, &old.instance.id)?;
            }
        }
        for instance in db::legacy_references(c, &p.id)? {
            if db::reviews(c, Some(&p.folder))?.iter().any(|r| {
                r.instance_id == instance
                    && r.person_id == id
                    && r.decision == PersonReviewDecision::Belongs
            }) {
                db::add_reference(c, &id, &instance)?;
            }
        }
        db::mark_migrated(c, &p.id, &id)?;
    }
    if !legacy.is_empty() {
        let mut targets = BTreeMap::<String, HashSet<String>>::new();
        for r in db::reviews(c, None)?
            .into_iter()
            .filter(|r| r.decision != PersonReviewDecision::DoesNotBelong)
        {
            targets
                .entry(r.instance_id)
                .or_default()
                .insert(r.person_id);
        }
        let tuples = db::tuples(c, None)?;
        for (instance, people) in targets {
            if people.len() == 1
                && tuples
                    .iter()
                    .any(|t| t.id == instance && t.person_id.is_none())
            {
                db::target(c, &instance, people.iter().next().expect("one identity"))?;
            }
        }
    }
    Ok(())
}

fn automatic_tuple(m: &PersonClusterMember) -> Result<PersonTuple, PeopleError> {
    Ok(PersonTuple {
        id: format!(
            "det:{}",
            hash(&(&m.asset_path, &m.instance_id, &m.source_revision))?
        ),
        asset_path: m.asset_path.clone(),
        source_revision: m.summary_revision.clone(),
        source_identity_revision: m.source_revision.clone(),
        face_box: Some(m.face_box),
        body_box: None,
        revision: 0,
        needs_review: false,
        person_id: None,
        decision: None,
        score: None,
    })
}

fn convert(snapshot: &PersonClusterSnapshot) -> Result<Suggestions, PeopleError> {
    let mut groups = Vec::new();
    for c in &snapshot.clusters {
        groups.push(PersonTupleGroup {
            id: c.id.clone(),
            person_id: None,
            members: c
                .members
                .iter()
                .map(automatic_tuple)
                .collect::<Result<_, _>>()?,
            cover: c.cover.clone(),
        });
    }
    for m in &snapshot.ungrouped {
        let t = automatic_tuple(m)?;
        groups.push(PersonTupleGroup {
            id: format!("single:{}", t.id),
            person_id: None,
            members: vec![t],
            cover: None,
        });
    }
    Ok(Suggestions {
        version: 1,
        run_id: snapshot.run_id.clone(),
        pipeline: snapshot.pipeline_fingerprint.clone(),
        groups,
        no_face_count: snapshot.no_face_count,
        unavailable_count: snapshot.unavailable_count,
        notice: None,
    })
}

fn load(c: &Connection, folder: &Path) -> Result<Option<Suggestions>, PeopleError> {
    let path = folder.to_string_lossy();
    let mut result: Option<Suggestions> = db::suggestions(c, &path)?
        .map(|j| serde_json::from_str(&j))
        .transpose()?;
    if result.is_none()
        && let Some(json) = repo::person_clusters::snapshot(c, &path)?
    {
        let old: PersonClusterSnapshot = serde_json::from_str(&json)?;
        result = Some(convert(&old)?);
    }
    let fingerprint = crate::pipeline_fingerprint(&crate::environment::catalog::manifest())
        .map_err(|e| failure(&e.to_string()))?;
    Ok(result.filter(|s| s.version == 1 && s.pipeline == fingerprint))
}

fn workspace(c: &Connection, folder: &Path) -> Result<FolderPeopleWorkspace, PeopleError> {
    let stored = load(c, folder)?;
    let manual = db::tuples(c, Some(&folder.to_string_lossy()))?;
    let reviews = db::reviews(c, Some(&folder.to_string_lossy()))?;
    let catalog = db::catalog(c)?;
    let revision = hash(&(&stored, &manual, &reviews, &catalog))?;
    let mut projected: BTreeMap<String, PersonTupleGroup> = BTreeMap::new();
    let mut included = HashSet::new();
    let mut source_groups = stored
        .as_ref()
        .map(|s| s.groups.clone())
        .unwrap_or_default();
    let mut by_asset: BTreeMap<_, BTreeMap<_, _>> = BTreeMap::new();
    for t in source_groups.iter().flat_map(|g| &g.members) {
        by_asset
            .entry(t.asset_path.clone())
            .or_default()
            .insert(t.id.clone(), t.clone());
    }
    let mut correspondence = HashMap::new();
    let mut conflicts = HashSet::new();
    for (path, detected) in by_asset {
        let Some(first) = detected.values().next() else {
            continue;
        };
        let anchors: Vec<_> = manual
            .iter()
            .filter(|m| m.asset_path == path)
            .map(|m| crate::alignment::ManualAnchor {
                id: m.id.clone(),
                source_identity_revision: (!m.source_identity_revision.is_empty())
                    .then(|| m.source_identity_revision.clone()),
                face_box: m.face_box,
                body_box: m.body_box,
                needs_review: m.needs_review,
            })
            .collect();
        let detections: Vec<_> = detected
            .values()
            .map(|t| crate::alignment::DetectedAnchor {
                id: t.id.clone(),
                source_identity_revision: t.source_identity_revision.clone(),
                face_box: t.face_box,
                body_box: t.body_box,
            })
            .collect();
        let aligned = crate::alignment::align_instances(
            &first.source_identity_revision,
            &anchors,
            &detections,
            crate::alignment::AlignmentPolicy::default(),
        )
        .map_err(|_| PeopleError::InvalidPersonInstance)?;
        for pair in aligned.matches {
            correspondence.insert(pair.detected_id, pair.manual_id);
        }
        if !aligned.conflicts.is_empty() {
            conflicts.extend(aligned.unmatched_detected_ids);
        }
    }
    // Unique source/geometry correspondence carries user decisions. Ambiguous
    // or stale anchors cannot be bypassed by creating another automatic tuple.
    for g in &mut source_groups {
        for t in &mut g.members {
            let matching: Vec<_> = manual
                .iter()
                .filter(|m| m.id == t.id || correspondence.get(&t.id) == Some(&m.id))
                .collect();
            if matching.len() == 1 {
                let old_person = t.person_id.clone();
                let score = t.score;
                *t = matching[0].clone();
                t.score = score;
                if t.person_id.is_none() {
                    t.person_id = old_person;
                }
            } else if matching.len() > 1 || conflicts.contains(&t.id) {
                t.needs_review = true;
            }
            included.insert(t.id.clone());
        }
    }
    for t in &manual {
        if !included.contains(&t.id) {
            source_groups.push(PersonTupleGroup {
                id: format!("manual:{}", t.id),
                person_id: None,
                members: vec![t.clone()],
                cover: None,
            });
        }
    }
    for group in source_groups {
        for mut t in group.members {
            if let Some(person) = &t.person_id {
                if let Some(review) = reviews
                    .iter()
                    .find(|r| r.instance_id == t.id && r.person_id == *person)
                {
                    t.decision = Some(review.decision);
                } else {
                    t.decision = Some(PersonReviewDecision::Pending);
                }
            }
            // Explicit exclusions remain accessible on the person's review group,
            // and the tuple returns to an unknown group for reassignment.
            if t.decision == Some(PersonReviewDecision::DoesNotBelong) {
                if let Some(person) = &t.person_id {
                    let key = format!("person:{person}");
                    projected
                        .entry(key.clone())
                        .or_insert_with(|| PersonTupleGroup {
                            id: key,
                            person_id: Some(person.clone()),
                            members: Vec::new(),
                            cover: group.cover.clone(),
                        })
                        .members
                        .push(t.clone());
                }
                t.person_id = None;
                t.decision = None;
            }
            let key = t.person_id.as_ref().map_or_else(
                || {
                    if group.person_id.is_some() {
                        format!("unknown:{}", group.id)
                    } else {
                        group.id.clone()
                    }
                },
                |p| format!("person:{p}"),
            );
            projected
                .entry(key.clone())
                .or_insert_with(|| PersonTupleGroup {
                    id: key,
                    person_id: t.person_id.clone(),
                    members: Vec::new(),
                    cover: group.cover.clone(),
                })
                .members
                .push(t);
        }
    }
    // An exclusion is a reviewable fact even after this tuple is reassigned.
    for review in reviews.iter().filter(|r| {
        r.decision == PersonReviewDecision::DoesNotBelong
            || manual
                .iter()
                .any(|t| t.id == r.instance_id && t.person_id.is_none())
    }) {
        if let Some(mut tuple) = manual.iter().find(|t| t.id == review.instance_id).cloned() {
            tuple.person_id = Some(review.person_id.clone());
            tuple.decision = Some(review.decision);
            let id = format!("person:{}", review.person_id);
            projected
                .entry(id.clone())
                .or_insert_with(|| PersonTupleGroup {
                    id,
                    person_id: Some(review.person_id.clone()),
                    members: Vec::new(),
                    cover: None,
                })
                .members
                .push(tuple);
        }
    }
    let mut groups: Vec<_> = projected.into_values().collect();
    for g in &mut groups {
        g.members.sort_by(|a, b| {
            b.score
                .unwrap_or(-1.0)
                .total_cmp(&a.score.unwrap_or(-1.0))
                .then(a.asset_path.cmp(&b.asset_path))
                .then(a.id.cmp(&b.id))
        });
        let mut seen = HashSet::new();
        g.members.retain(|t| seen.insert(t.id.clone()));
    }
    groups.sort_by(|a, b| {
        b.person_id
            .is_some()
            .cmp(&a.person_id.is_some())
            .then(b.members.len().cmp(&a.members.len()))
            .then(a.id.cmp(&b.id))
    });
    let unknown_count = groups.iter().filter(|g| g.person_id.is_none()).count();
    let known_count = groups
        .iter()
        .filter(|g| {
            g.person_id.is_some()
                && g.members
                    .iter()
                    .any(|t| t.decision != Some(PersonReviewDecision::DoesNotBelong))
        })
        .count();
    Ok(FolderPeopleWorkspace {
        folder_path: folder.into(),
        revision,
        groups,
        unknown_count,
        known_count,
        no_face_count: stored.as_ref().map_or(0, |s| s.no_face_count),
        unavailable_count: stored.as_ref().map_or(0, |s| s.unavailable_count),
        has_analysis: stored.is_some(),
        notice: stored.and_then(|s| s.notice),
    })
}

impl People {
    pub fn global_person_reference_tuples(
        &self,
        id: &str,
    ) -> Result<Vec<PersonTuple>, PeopleError> {
        let c = self.store.read();
        let person = db::catalog(&c)?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or(PeopleError::MissingPersonRecord)?;
        Ok(db::tuples(&c, None)?
            .into_iter()
            .filter(|t| person.reference_instance_ids.contains(&t.id))
            .collect())
    }
    pub fn has_current_person_features(&self, folder: &Path) -> Result<bool, PeopleError> {
        let c = self.store.read();
        let Some((_, id)) = repo::person_cache::analysis_head(&c, &folder.to_string_lossy())?
        else {
            return Ok(false);
        };
        let fingerprint = crate::pipeline_fingerprint(&crate::environment::catalog::manifest())
            .map_err(|e| failure(&e.to_string()))?;
        Ok(repo::person_cache::analysis_run(&c, &id)?.is_some_and(|r| {
            r.state == PersonAnalysisState::Completed && r.pipeline_fingerprint == fingerprint
        }))
    }
    pub fn initialize_global_people(&self) -> Result<(), PeopleError> {
        if !db::migration_needed(&self.store.read())? {
            return Ok(());
        }
        let mut c = self.store.write();
        let tx = c.transaction().map_err(StoreError::from)?;
        migrate(&tx)?;
        tx.commit().map_err(StoreError::from)?;
        Ok(())
    }
    pub fn global_people(&self) -> Result<Vec<GlobalPerson>, PeopleError> {
        self.initialize_global_people()?;
        Ok(db::catalog(&self.store.read())?)
    }
    pub fn global_person_gallery(
        &self,
        person: &str,
        offset: u32,
        limit: u32,
    ) -> Result<GlobalPersonGallery, PeopleError> {
        self.initialize_global_people()?;
        let mut c = self.store.read();
        let tx = c.transaction().map_err(StoreError::from)?;
        let (total, folder_count) = db::confirmed_gallery_counts(&tx, person)?;
        let tuples = db::confirmed_gallery(&tx, person, offset, limit.clamp(1, 60))?;
        tx.commit().map_err(StoreError::from)?;
        Ok(GlobalPersonGallery {
            tuples,
            total,
            folder_count,
        })
    }
    pub fn global_person_folders(
        &self,
        person: &str,
        offset: u32,
        limit: u32,
    ) -> Result<GlobalPersonFolders, PeopleError> {
        self.initialize_global_people()?;
        let mut c = self.store.read();
        let tx = c.transaction().map_err(StoreError::from)?;
        let (_, total) = db::confirmed_gallery_counts(&tx, person)?;
        let folders = db::confirmed_folders(&tx, person, offset, limit.clamp(1, 60))?;
        tx.commit().map_err(StoreError::from)?;
        Ok(GlobalPersonFolders { folders, total })
    }
    pub fn folder_people_workspace(
        &self,
        folder: &Path,
    ) -> Result<FolderPeopleWorkspace, PeopleError> {
        self.initialize_global_people()?;
        workspace(&self.store.read(), folder)
    }
    pub fn filter_assets_by_tuple(
        &self,
        folder: &Path,
        assets: &[AssetSummary],
        filter: &PersonTupleFilter,
    ) -> Result<Vec<AssetSummary>, PeopleError> {
        let w = self.folder_people_workspace(folder)?;
        let versions: HashSet<_> = w
            .groups
            .iter()
            .filter(|g| g.id == filter.group_id)
            .flat_map(|g| &g.members)
            .filter(|t| filter.decision.is_none_or(|d| t.decision == Some(d)))
            .map(|t| (t.asset_path.clone(), t.source_revision.clone()))
            .collect();
        Ok(assets
            .iter()
            .filter(|a| {
                versions.contains(&(
                    a.path.clone(),
                    format!("{}:{}", a.size_bytes, a.modified_at_ms),
                ))
            })
            .cloned()
            .collect())
    }
}
