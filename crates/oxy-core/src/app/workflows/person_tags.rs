//! Reconcile person claims without changing manual or sidecar sources.
use crate::people::{
    PeopleError,
    global_people::requests::{payload, record, replay},
    repo::global as db,
    types::SetGlobalPersonTag,
};
use crate::storage::{Connection, Store, StoreError};
use std::collections::{BTreeSet, HashMap};

const PERSON_SOURCE: &str = "person";

pub(crate) fn set_global_person_tag(
    store: &Store,
    input: &SetGlobalPersonTag,
) -> Result<(), PeopleError> {
    let payload = payload("globalTag", input)?;
    let mut connection = store.write();
    let transaction = connection.transaction().map_err(StoreError::from)?;
    if let Some(result) = replay(&transaction, &input.request_id, &payload)? {
        return Ok(result);
    }
    if !db::catalog(&transaction)?
        .iter()
        .any(|person| person.id == input.person_id && person.revision == input.expected_revision)
    {
        return Err(PeopleError::PersonConflict);
    }
    if let Some(tag) = input.tag_id
        && !crate::tags::repo::tag_exists(&transaction, tag)?
    {
        return Err(PeopleError::MissingTagParent);
    }
    db::set_tag(&transaction, &input.person_id, input.tag_id)?;
    db::bump(&transaction, &input.person_id)?;
    reconcile_global_person_sources(&transaction, &input.person_id)?;
    record(&transaction, &input.request_id, &payload, &())?;
    transaction.commit().map_err(StoreError::from)?;
    Ok(())
}

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
        .collect::<BTreeSet<_>>();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::people::{People, repo::manual, types::*};
    use std::sync::Arc;

    #[test]
    fn global_tag_transaction_rolls_back_all_features_and_replays_once() {
        let store = Arc::new(Store::in_memory().unwrap());
        let people = People::new(Arc::clone(&store));
        let person = people
            .save_global_person(&SaveGlobalPerson {
                id: None,
                display_name: "Ada".into(),
                expected_revision: 0,
                request_id: "create-person".into(),
            })
            .unwrap();
        let tag = {
            let connection = store.write();
            let tag = crate::tags::repo::insert_tag(&connection, None, "Ada", "ada", 0).unwrap();
            manual::insert_instance(
                &connection,
                &manual::NewInstance {
                    id: "instance",
                    folder_path: "/photos",
                    asset_path: "/photos/photo.jpg",
                    source_revision: "1:1",
                    source_identity_revision: Some("identity"),
                    face_box_json: Some("[0.1,0.1,0.2,0.2]"),
                    body_box_json: None,
                },
            )
            .unwrap();
            db::review(
                &connection,
                "instance",
                &person.id,
                PersonReviewDecision::Belongs,
            )
            .unwrap();
            connection
                .execute_batch(
                    "CREATE TRIGGER reject_person_event BEFORE INSERT ON global_person_events
                 BEGIN SELECT RAISE(ABORT, 'event write failed'); END;",
                )
                .unwrap();
            tag
        };
        let input = SetGlobalPersonTag {
            person_id: person.id.clone(),
            tag_id: Some(tag),
            expected_revision: person.revision,
            request_id: "set-tag".into(),
        };
        assert!(set_global_person_tag(&store, &input).is_err());
        let after = people.global_people().unwrap().into_iter().next().unwrap();
        assert_eq!(after.revision, person.revision);
        assert_eq!(after.tag_id, None);
        {
            let connection = store.read();
            assert!(
                crate::tags::repo::source_tag_ids_of(
                    &connection,
                    "/photos/photo.jpg",
                    PERSON_SOURCE,
                    &format!("global:{}", person.id),
                )
                .unwrap()
                .is_empty()
            );
            assert!(
                !crate::tags::repo::effective_assignment_present(
                    &connection,
                    "/photos/photo.jpg",
                    tag,
                )
                .unwrap()
            );
            assert!(
                crate::tags::repo::pending_sync_paths(&connection)
                    .unwrap()
                    .is_empty()
            );
            assert!(db::event(&connection, &input.request_id).unwrap().is_none());
        }
        store
            .write()
            .execute_batch("DROP TRIGGER reject_person_event;")
            .unwrap();
        set_global_person_tag(&store, &input).unwrap();
        set_global_person_tag(&store, &input).unwrap();
        let after = people.global_people().unwrap().into_iter().next().unwrap();
        assert_eq!(after.revision, person.revision + 1);
        assert_eq!(after.tag_id, Some(tag));
        assert!(
            crate::tags::repo::effective_assignment_present(
                &store.read(),
                "/photos/photo.jpg",
                tag,
            )
            .unwrap()
        );
        let changed_payload = SetGlobalPersonTag {
            tag_id: None,
            ..input
        };
        assert!(matches!(
            set_global_person_tag(&store, &changed_payload),
            Err(PeopleError::PersonConflict)
        ));
    }
}
