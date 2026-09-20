//! Durable per-asset synchronization state and conservative three-way reconciliation.
use crate::people::PeopleDocument;
use oxy_domain::{
    DecisionRecord, FaceSidecarState, PersonRecord, PortableFaceFact, PortableFaceFacts,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

fn same_region(a: oxy_domain::NormalizedRect, b: oxy_domain::NormalizedRect) -> bool {
    [a.x - b.x, a.y - b.y, a.width - b.width, a.height - b.height]
        .iter()
        .all(|difference| difference.abs() < 1e-6)
}

pub(crate) fn refresh_desired(document: &mut PeopleDocument) {
    let mut paths: BTreeSet<_> = document.sidecars.keys().cloned().collect();
    paths.extend(
        document
            .decisions
            .iter()
            .map(|record| record.asset_path.clone()),
    );
    for path in paths {
        let state = document.sidecars.entry(path.clone()).or_default();
        let before = state.desired.clone();
        let mut used = BTreeSet::new();
        for record in document
            .decisions
            .iter()
            .filter(|record| record.asset_path == path)
        {
            let person = record.decision.person_id().and_then(|id| {
                document
                    .persons
                    .iter()
                    .find(|person| person.person_id == id)
            });
            let name = person.map(|person| person.display_name.clone());
            let tag_path = person.and_then(|person| person.tag_path.clone());
            if let Some(fact) = state
                .desired
                .facts
                .iter_mut()
                .find(|fact| same_region(fact.region, record.region))
            {
                if fact.decision.as_ref() != Some(&record.decision)
                    || fact.person_name != name
                    || fact.person_tag_path != tag_path
                {
                    fact.decision = Some(record.decision.clone());
                    fact.person_name = name;
                    fact.person_tag_path = tag_path;
                    fact.revision = uuid::Uuid::new_v4().to_string();
                }
                used.insert(fact.id.clone());
            } else {
                let id = uuid::Uuid::new_v4().to_string();
                used.insert(id.clone());
                state.desired.facts.push(PortableFaceFact {
                    person_tag_path: tag_path,
                    id,
                    revision: uuid::Uuid::new_v4().to_string(),
                    region: record.region,
                    decision: Some(record.decision.clone()),
                    person_name: name,
                });
            }
        }
        for fact in &mut state.desired.facts {
            if !used.contains(&fact.id) && fact.decision.is_some() {
                fact.decision = None;
                fact.person_name = None;
                fact.person_tag_path = None;
                fact.revision = uuid::Uuid::new_v4().to_string();
            }
        }
        state.desired.facts.sort_by(|a, b| a.id.cmp(&b.id));
        if state.desired != before {
            state.last_error = None;
        }
    }
}

/// Conflicts never use wall-clock last-writer-wins.
pub fn merge_face_facts(
    base: &PortableFaceFacts,
    local: &PortableFaceFacts,
    remote: &PortableFaceFacts,
) -> Option<PortableFaceFacts> {
    let map = |facts: &PortableFaceFacts| {
        facts
            .facts
            .iter()
            .map(|fact| (fact.id.clone(), fact.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let base = map(base);
    let local = map(local);
    let remote = map(remote);
    let keys: BTreeSet<_> = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .cloned()
        .collect();
    let mut facts = Vec::new();
    for key in keys {
        let (b, l, r) = (base.get(&key), local.get(&key), remote.get(&key));
        let selected = if l == r || r == b {
            l
        } else if l == b {
            r
        } else {
            return None;
        };
        if let Some(fact) = selected {
            facts.push(fact.clone());
        }
    }
    for (index, left) in facts.iter().enumerate() {
        for right in facts.iter().skip(index + 1) {
            if left.decision.is_some()
                && right.decision.is_some()
                && left.region.iou(right.region) >= 0.5
            {
                return None;
            }
        }
    }
    Some(PortableFaceFacts { facts })
}

pub(crate) fn import_facts(document: &mut PeopleDocument, path: &Path, facts: &PortableFaceFacts) {
    let previous: Vec<_> = document
        .decisions
        .iter()
        .filter(|record| record.asset_path == path)
        .cloned()
        .collect();
    document
        .decisions
        .retain(|record| record.asset_path != path);
    for fact in &facts.facts {
        let Some(decision) = &fact.decision else {
            continue;
        };
        if let Some(id) = decision.person_id() {
            let name = fact.person_name.clone().unwrap_or_else(|| id.to_owned());
            if let Some(person) = document
                .persons
                .iter_mut()
                .find(|person| person.person_id == id)
            {
                person.display_name = name;
                if let Some(path) = &fact.person_tag_path {
                    person.tag_path = Some(path.clone());
                    person.linked_tag_id = None;
                }
            } else {
                document.persons.push(PersonRecord {
                    tag_path: fact.person_tag_path.clone(),
                    person_id: id.into(),
                    display_name: name,
                    linked_tag_id: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                });
            }
        }
        let old = previous.iter().find(|record| record.region == fact.region);
        document.decisions.push(DecisionRecord {
            observation_id: old.and_then(|record| record.observation_id.clone()),
            asset_id: oxy_fs::stable_asset_id(path),
            asset_path: path.into(),
            region: fact.region,
            decision: decision.clone(),
            created_at_ms: old.map_or(0, |record| record.created_at_ms),
            proposed_similarity: old.and_then(|record| record.proposed_similarity),
        });
    }
}

pub(crate) fn merge_remote(
    document: &mut PeopleDocument,
    path: &Path,
    remote: PortableFaceFacts,
) -> bool {
    // A per-photo copy cannot silently rename an existing global person. The
    // user must resolve this ambiguity, including stale copies from other photos.
    let name_conflict = remote.facts.iter().any(|fact| {
        fact.decision
            .as_ref()
            .and_then(|decision| decision.person_id())
            .zip(fact.person_name.as_ref())
            .is_some_and(|(id, name)| {
                document.persons.iter().any(|person| {
                    person.person_id == id
                        && (person.display_name != *name
                            || fact
                                .person_tag_path
                                .as_ref()
                                .is_some_and(|path| person.tag_path.as_ref() != Some(path)))
                })
            })
    });
    let state = document.sidecars.entry(path.into()).or_default();
    if name_conflict && remote != state.base {
        state.conflict = Some(remote);
        return false;
    }
    if let Some(merged) = merge_face_facts(&state.base, &state.desired, &remote) {
        let changed = merged != state.desired;
        state.base = remote;
        state.desired = merged.clone();
        state.conflict = None;
        state.last_error = None;
        if changed {
            import_facts(document, path, &merged);
        }
        changed
    } else {
        state.conflict = Some(remote);
        false
    }
}

pub(crate) fn pending(state: &FaceSidecarState) -> bool {
    state.base != state.desired
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fact(id: &str, revision: &str, x: f32) -> PortableFaceFact {
        PortableFaceFact {
            person_tag_path: None,
            id: id.into(),
            revision: revision.into(),
            region: oxy_domain::NormalizedRect {
                x,
                y: 0.1,
                width: 0.1,
                height: 0.1,
            },
            decision: Some(oxy_domain::FaceDecision::NotFace),
            person_name: None,
        }
    }
    #[test]
    fn pending_annotations_survive_reload_and_names_require_conflict_resolution() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("people.json");
        let asset = Path::new("/photos/a.jpg");
        let (store, _) = crate::PersonStore::load(&path).unwrap();
        let mut remote = PortableFaceFacts {
            facts: vec![fact("a", "r1", 0.1)],
        };
        remote.facts[0].decision = Some(oxy_domain::FaceDecision::ConfirmPerson {
            person_id: "alice".into(),
        });
        remote.facts[0].person_name = Some("Alice".into());
        assert!(store.merge_sidecar(asset, remote.clone()).unwrap());
        let mut person = store.person("alice").unwrap();
        person.display_name = "Alicia".into();
        store.upsert_person(person).unwrap();
        drop(store);
        let (store, _) = crate::PersonStore::load(&path).unwrap();
        assert!(store.sidecar_status()[0].pending);
        assert!(
            !store
                .merge_sidecar(Path::new("/photos/b.jpg"), remote)
                .unwrap()
        );
        assert_eq!(store.person("alice").unwrap().display_name, "Alicia");
        assert!(
            store
                .sidecar_state(Path::new("/photos/b.jpg"))
                .unwrap()
                .conflict
                .is_some()
        );
        store
            .resolve_sidecar(Path::new("/photos/b.jpg"), true)
            .unwrap();
        assert_eq!(store.person("alice").unwrap().display_name, "Alice");
        let copy = Path::new("/photos/copy.jpg");
        store.copy_decisions(asset, copy, "copy-asset").unwrap();
        assert_eq!(
            store.sidecar_state(asset).unwrap(),
            store.sidecar_state(copy).unwrap()
        );
        store.remove_decisions(copy).unwrap();
        assert!(store.sidecar_state(copy).is_none());
        store.delete_all_annotations().unwrap();
        assert!(store.persons().is_empty());
        assert!(store.decisions().is_empty());
        assert!(
            store
                .sidecar_state(asset)
                .unwrap()
                .desired
                .facts
                .iter()
                .all(|fact| fact.decision.is_none())
        );
    }
    #[test]
    fn merges_independent_edits_but_retains_conflicts() {
        let base = PortableFaceFacts {
            facts: vec![fact("a", "a1", 0.1), fact("b", "b1", 0.5)],
        };
        let mut local = base.clone();
        local.facts[0].revision = "a2".into();
        let mut remote = base.clone();
        remote.facts[1].revision = "b2".into();
        let merged = merge_face_facts(&base, &local, &remote).unwrap();
        assert_eq!(merged.facts[0].revision, "a2");
        assert_eq!(merged.facts[1].revision, "b2");
        remote.facts[0].revision = "a3".into();
        assert!(merge_face_facts(&base, &local, &remote).is_none());
    }
    #[test]
    fn deletion_tombstone_survives_an_unchanged_remote_copy() {
        let base = PortableFaceFacts {
            facts: vec![fact("a", "a1", 0.1)],
        };
        let mut local = base.clone();
        local.facts[0].decision = None;
        local.facts[0].revision = "deleted".into();
        assert_eq!(merge_face_facts(&base, &local, &base), Some(local));
    }
}
