//! Cache-deletion durability.
//!
//! The project treats SQLite as a rebuildable cache: deleting it may cost
//! decode time and nothing else. Confirmations are not rebuildable, so this
//! test destroys the cache completely and checks that the people data comes
//! back from the durable file and re-attaches to a re-analyzed library.
//!
//! This is the acceptance criterion from the research report:
//!
//! > 删除全部 SQLite cache 后重新打开图库，人工确认的人物关系能够从
//! > sidecar/项目用户数据重新恢复

use std::path::{Path, PathBuf};

use oxy_domain::{
    DecisionRecord, FaceDecision, FaceObservation, FaceReviewFilter, FaceReviewState,
    NormalizedRect, PersonRecord,
};
use oxy_library::{Library, StoredFace};

const DETECTOR: &str = "yunet/durability/detect-v1";
const EMBEDDER: &str = "sface/durability/align-v1";

fn rect(x: f32, y: f32, width: f32, height: f32) -> NormalizedRect {
    NormalizedRect::new(x, y, width, height)
}

fn observation_at(asset: &str, id: &str, bbox: NormalizedRect) -> FaceObservation {
    FaceObservation {
        observation_id: id.into(),
        asset_id: asset.into(),
        asset_path: PathBuf::from(asset),
        source_revision: "100:200:faces-v1".into(),
        local_index: 0,
        bbox,
        landmarks: Vec::new(),
        detection_score: 0.97,
        detector_fingerprint: DETECTOR.into(),
    }
}

fn store_faces(library: &Library, faces: &[(&str, NormalizedRect)]) {
    store_faces_at(library, "/photos/a.jpg", faces);
}

fn store_faces_at(library: &Library, asset: &str, faces: &[(&str, NormalizedRect)]) {
    let stored: Vec<StoredFace> = faces
        .iter()
        .map(|(id, bbox)| StoredFace {
            observation: observation_at(asset, id, *bbox),
            embedding: vec![1.0, 0.0, 0.0],
        })
        .collect();
    library
        .replace_asset_faces(
            Path::new(asset),
            asset,
            "100:200:faces-v1",
            DETECTOR,
            EMBEDDER,
            &stored,
        )
        .expect("faces stored");
}

#[test]
fn confirmations_survive_deleting_the_entire_sqlite_cache() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("oxyviewer.sqlite");
    let store_path = directory.path().join("people.json");

    // --- A library with real user data -------------------------------------
    {
        let library = Library::open(&database).expect("library opens");
        store_faces(&library, &[("face-1", rect(0.10, 0.10, 0.20, 0.20))]);
        library
            .create_person("person-alice", "Alice", None)
            .expect("person");
        library
            .record_face_decision(
                "face-1",
                &FaceDecision::ConfirmPerson {
                    person_id: "person-alice".into(),
                },
                None,
            )
            .expect("confirm");

        // First run after the upgrade: the store does not exist yet, so it is
        // seeded from the user data already in the cache.
        let (store, origin) = oxy_userdata::PersonStore::load(&store_path).expect("store");
        assert_eq!(origin, oxy_userdata::StoreOrigin::Created);
        let (persons, decisions) = library.export_user_data().expect("export");
        assert_eq!(persons.len(), 1);
        assert_eq!(decisions.len(), 1);
        store.replace_all(persons, decisions).expect("seeded");
    }
    assert!(store_path.is_file(), "the durable file now exists");

    // --- The user deletes the rebuildable cache ----------------------------
    std::fs::remove_file(&database).expect("cache deleted");
    assert!(!database.exists());

    // --- Reopen: the people data must come back from the durable file ------
    let library = Library::open(&database).expect("library reopens");
    assert!(
        library.persons().expect("persons").is_empty(),
        "a fresh cache starts empty"
    );
    let (store, origin) = oxy_userdata::PersonStore::load(&store_path).expect("store reloads");
    assert_eq!(origin, oxy_userdata::StoreOrigin::Loaded);
    let persons = store.persons();
    let decisions = store.decisions();
    assert_eq!(persons.len(), 1);
    assert_eq!(decisions.len(), 1);
    library
        .replace_user_data(&persons, &decisions)
        .expect("projected");

    let restored = library.persons().expect("persons");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].person_id, "person-alice");
    assert_eq!(restored[0].display_name, "Alice");
    assert_eq!(
        restored[0].face_count, 1,
        "the confirmation is durable user data, so it counts even before the \
         cache is rebuilt"
    );
    assert!(
        restored[0].cover_observation_id.is_none(),
        "the cover face is cache-bound and is restored by the next analysis"
    );

    // The decision survived, still bound to the region and asset it was made
    // on. There is no observation to show in the review queue yet.
    let decisions = library.face_decisions().expect("decisions");
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].decision,
        FaceDecision::ConfirmPerson {
            person_id: "person-alice".into()
        }
    );
    assert_eq!(decisions[0].asset_path, Path::new("/photos/a.jpg"));
    assert!(
        library
            .face_review_page(FaceReviewFilter::All, 0, 10)
            .expect("review")
            .items
            .is_empty(),
        "the queue is empty until the cache is rebuilt"
    );

    // --- Re-analysis re-attaches the confirmation to the new observation ---
    store_faces(
        &library,
        &[("face-1-new", rect(0.104, 0.098, 0.204, 0.198))],
    );
    let decisions = library.face_decisions().expect("decisions");
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].observation_id.as_deref(),
        Some("face-1-new"),
        "the confirmation follows the region, not the old detector id"
    );
    let confirmed = library
        .face_review_page(FaceReviewFilter::Confirmed, 0, 10)
        .expect("confirmed page");
    assert_eq!(confirmed.items.len(), 1);
    assert_eq!(confirmed.items[0].state, FaceReviewState::Confirmed);
    assert_eq!(
        confirmed.items[0].confirmed_person_name.as_deref(),
        Some("Alice")
    );
    assert_eq!(library.persons().expect("persons")[0].face_count, 1);
}

#[test]
fn the_projection_always_matches_the_durable_store() {
    // A durable write that is followed by a projection means the cache can
    // never hold a person or decision the store does not.
    let directory = tempfile::tempdir().unwrap();
    let library = Library::in_memory().expect("library");
    let (store, _) = oxy_userdata::PersonStore::load(directory.path().join("people.json")).unwrap();

    let now = 1_700_000_000_000u64;
    store
        .upsert_person(PersonRecord {
            person_id: "p1".into(),
            display_name: "Alice".into(),
            linked_tag_id: None,
            created_at_ms: now,
            updated_at_ms: now,
        })
        .unwrap();
    store
        .set_decision(DecisionRecord {
            observation_id: Some("obs-1".into()),
            asset_id: "asset-1".into(),
            asset_path: PathBuf::from("/photos/a.jpg"),
            region: rect(0.1, 0.1, 0.2, 0.2),
            decision: FaceDecision::NotFace,
            created_at_ms: now,
            proposed_similarity: None,
        })
        .unwrap();
    library
        .replace_user_data(&store.persons(), &store.decisions())
        .unwrap();

    assert_eq!(library.persons().unwrap().len(), 1);
    assert_eq!(library.face_decisions().unwrap().len(), 1);

    // Renaming through the store and re-projecting leaves no stale name behind.
    store.rename_person("p1", "Alice Zhang", now + 1).unwrap();
    library
        .replace_user_data(&store.persons(), &store.decisions())
        .unwrap();
    assert_eq!(library.persons().unwrap()[0].display_name, "Alice Zhang");

    // Removing the person removes its decisions from both sides, and the
    // not-face decision that did not name anyone is untouched.
    store.remove_person("p1").unwrap();
    library
        .replace_user_data(&store.persons(), &store.decisions())
        .unwrap();
    assert!(library.persons().unwrap().is_empty());
    let decisions = library.face_decisions().unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].decision, FaceDecision::NotFace);
}

#[test]
fn a_rejection_survives_the_cache_and_still_blocks_the_proposal() {
    let directory = tempfile::tempdir().unwrap();
    let store_path = directory.path().join("people.json");
    let (store, _) = oxy_userdata::PersonStore::load(&store_path).unwrap();
    let now = 1_700_000_000_000u64;
    store
        .upsert_person(PersonRecord {
            person_id: "p1".into(),
            display_name: "Alice".into(),
            linked_tag_id: None,
            created_at_ms: now,
            updated_at_ms: now,
        })
        .unwrap();
    store
        .set_decision(DecisionRecord {
            observation_id: Some("obs-1".into()),
            asset_id: "asset-1".into(),
            asset_path: PathBuf::from("/photos/a.jpg"),
            region: rect(0.1, 0.1, 0.2, 0.2),
            decision: FaceDecision::RejectPerson {
                person_id: "p1".into(),
            },
            created_at_ms: now,
            proposed_similarity: Some(0.44),
        })
        .unwrap();

    // Delete the cache, rebuild it, and project the rejection from the store.
    let fresh = Library::in_memory().unwrap();
    store_faces(&fresh, &[("obs-1", rect(0.1, 0.1, 0.2, 0.2))]);
    fresh
        .replace_user_data(&store.persons(), &store.decisions())
        .unwrap();
    // Cached proposals are gone with the cache; the durable rejection is what
    // keeps a fresh proposal from coming back.
    fresh
        .replace_face_candidates(
            "matcher-v1",
            &[oxy_domain::FaceCandidate {
                observation_id: "obs-1".into(),
                person_id: "p1".into(),
                person_name: "Alice".into(),
                similarity: 0.99,
                matcher_fingerprint: "matcher-v1".into(),
            }],
        )
        .unwrap();
    let rejected = fresh
        .face_review_page(FaceReviewFilter::Rejected, 0, 10)
        .expect("rejected");
    assert_eq!(rejected.items.len(), 1);
    assert_eq!(rejected.items[0].state, FaceReviewState::Rejected);
    assert!(
        fresh
            .face_review_page(FaceReviewFilter::Pending, 0, 10)
            .expect("pending")
            .items
            .is_empty(),
        "a durable rejection must outrank a fresh machine proposal"
    );
}

#[test]
fn person_operations_and_their_undo_survive_a_cache_wipe() {
    // The undo journal is user data: a cache deletion must not silently turn a
    // reversible merge into an irreversible one.
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("oxyviewer.sqlite");
    let store_path = directory.path().join("people.json");

    {
        let library = Library::open(&database).unwrap();
        store_faces(
            &library,
            &[
                ("obs-1", rect(0.10, 0.10, 0.20, 0.20)),
                ("obs-2", rect(0.40, 0.10, 0.20, 0.20)),
                ("obs-3", rect(0.70, 0.10, 0.20, 0.20)),
            ],
        );
        library.create_person("alice", "Alice", None).unwrap();
        library.create_person("alica", "Alica", None).unwrap();
        for (observation, person) in [("obs-1", "alice"), ("obs-2", "alice"), ("obs-3", "alica")] {
            library
                .record_face_decision(
                    observation,
                    &FaceDecision::ConfirmPerson {
                        person_id: person.into(),
                    },
                    None,
                )
                .unwrap();
        }

        let (store, _) = oxy_userdata::PersonStore::load(&store_path).unwrap();
        let (persons, decisions) = library.export_user_data().unwrap();
        store.replace_all(persons, decisions).unwrap();

        // Alice + Alica were always one person: merge and keep the undo.
        let moved = store.merge_persons("alica", "alice").unwrap();
        assert_eq!(moved, 1);
        library
            .replace_user_data(&store.persons(), &store.decisions())
            .unwrap();
        assert_eq!(library.persons().unwrap().len(), 1);
        assert_eq!(library.persons().unwrap()[0].face_count, 3);
    }

    // Delete every rebuildable byte and start over from the durable file.
    std::fs::remove_file(&database).unwrap();
    let library = Library::open(&database).unwrap();
    let (store, origin) = oxy_userdata::PersonStore::load(&store_path).unwrap();
    assert_eq!(origin, oxy_userdata::StoreOrigin::Loaded);
    library
        .replace_user_data(&store.persons(), &store.decisions())
        .unwrap();
    assert_eq!(
        library.persons().unwrap().len(),
        1,
        "the merge is part of the durable state"
    );

    let undoable = store.undoable().expect("the journal survived the wipe");
    assert_eq!(undoable.kind, "mergePersons");
    assert_eq!(undoable.face_count, 1);
    assert_eq!(undoable.other_person_name.as_deref(), Some("Alica"));

    // Undoing restores both people and re-points the faces.
    store.undo().unwrap();
    library
        .replace_user_data(&store.persons(), &store.decisions())
        .unwrap();
    let persons = library.persons().unwrap();
    assert_eq!(persons.len(), 2);
    assert_eq!(
        persons
            .iter()
            .find(|person| person.display_name == "Alice")
            .unwrap()
            .face_count,
        2,
        "Alice keeps her own photos"
    );
    assert_eq!(
        persons
            .iter()
            .find(|person| person.display_name == "Alica")
            .unwrap()
            .face_count,
        1
    );
    assert!(store.undoable().is_none(), "each operation undoes once");
}

#[test]
fn detaching_faces_returns_them_to_the_unknown_queue() {
    let directory = tempfile::tempdir().unwrap();
    let library = Library::open(&directory.path().join("oxyviewer.sqlite")).unwrap();
    let (store, _) = oxy_userdata::PersonStore::load(directory.path().join("people.json")).unwrap();

    store_faces(
        &library,
        &[
            ("obs-1", rect(0.10, 0.10, 0.20, 0.20)),
            ("obs-2", rect(0.40, 0.10, 0.20, 0.20)),
        ],
    );
    store
        .upsert_person(PersonRecord {
            person_id: "alice".into(),
            display_name: "Alice".into(),
            linked_tag_id: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    for observation in ["obs-1", "obs-2"] {
        store
            .set_decision(DecisionRecord {
                observation_id: Some(observation.into()),
                asset_id: "/photos/a.jpg".into(),
                asset_path: PathBuf::from("/photos/a.jpg"),
                region: rect(0.1, 0.1, 0.2, 0.2),
                decision: FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
                created_at_ms: 1,
                proposed_similarity: None,
            })
            .unwrap();
    }
    library
        .replace_user_data(&store.persons(), &store.decisions())
        .unwrap();

    // One wrongly attached face is detached; the other stays with Alice.
    let removed = store
        .remove_faces_from_person("alice", &["obs-2".to_string()])
        .unwrap();
    assert_eq!(removed, 1);
    library
        .replace_user_data(&store.persons(), &store.decisions())
        .unwrap();

    let persons = library.persons().unwrap();
    assert_eq!(
        persons.len(),
        1,
        "detaching a face is not deleting a person"
    );
    assert_eq!(persons[0].face_count, 1, "Alice keeps her other photo");
    let unknown = library
        .face_review_page(FaceReviewFilter::Unknown, 0, 10)
        .expect("unknown page");
    assert_eq!(unknown.items.len(), 1);
    assert_eq!(unknown.items[0].observation_id, "obs-2");
}

#[test]
fn a_rename_moves_the_confirmation_and_it_survives_a_cache_wipe() {
    // The report's identity-stability gate: renaming a photo must not make the
    // person disappear from it.
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("oxyviewer.sqlite");
    let store_path = directory.path().join("people.json");
    let source = "/photos/IMG_1234.jpg";
    let destination = "/photos/Tokyo-2026-001.jpg";

    {
        let library = Library::open(&database).unwrap();
        store_faces_at(&library, source, &[("obs-1", rect(0.10, 0.10, 0.20, 0.20))]);
        library
            .create_person("alice", "Alice", None)
            .expect("person");
        library
            .record_face_decision(
                "obs-1",
                &FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
                None,
            )
            .expect("confirm");
        let (store, _) = oxy_userdata::PersonStore::load(&store_path).unwrap();
        let (persons, decisions) = library.export_user_data().unwrap();
        store.replace_all(persons, decisions).unwrap();

        // The user renames the file.
        let asset_id = format!("id:{destination}");
        assert_eq!(
            store
                .move_decisions(Path::new(source), Path::new(destination), &asset_id)
                .unwrap(),
            1
        );
        library
            .move_asset_face_state(Path::new(source), Path::new(destination))
            .unwrap();
        library
            .replace_user_data(&store.persons(), &store.decisions())
            .unwrap();

        // The renamed asset shows the confirmation immediately, and the old
        // path is gone rather than duplicated.
        let confirmed = library
            .face_review_page(FaceReviewFilter::Confirmed, 0, 10)
            .expect("confirmed page");
        assert_eq!(confirmed.items.len(), 1);
        assert_eq!(confirmed.items[0].asset_path, Path::new(destination));
        assert!(
            library
                .face_decisions()
                .unwrap()
                .iter()
                .all(|record| record.asset_path == Path::new(destination)),
            "no decision may be left on the old path"
        );
    }

    // A cache wipe must not undo the rename.
    std::fs::remove_file(&database).unwrap();
    let library = Library::open(&database).unwrap();
    let (store, origin) = oxy_userdata::PersonStore::load(&store_path).unwrap();
    assert_eq!(origin, oxy_userdata::StoreOrigin::Loaded);
    library
        .replace_user_data(&store.persons(), &store.decisions())
        .unwrap();
    let decisions = library.face_decisions().unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].asset_path,
        Path::new(destination),
        "the durable store remembers the new path"
    );
    assert_eq!(library.persons().unwrap()[0].face_count, 1);

    // Re-analysis at the new path re-attaches the confirmation to its new
    // observation without asking the user again.
    store_faces_at(
        &library,
        destination,
        &[("obs-new", rect(0.10, 0.10, 0.20, 0.20))],
    );
    assert_eq!(
        library.face_decisions().unwrap()[0]
            .observation_id
            .as_deref(),
        Some("obs-new")
    );
    assert_eq!(
        library
            .face_review_page(FaceReviewFilter::Confirmed, 0, 10)
            .unwrap()
            .items[0]
            .confirmed_person_name
            .as_deref(),
        Some("Alice")
    );
}

#[test]
fn deleting_an_asset_drops_its_face_data() {
    let directory = tempfile::tempdir().unwrap();
    let library = Library::open(&directory.path().join("oxyviewer.sqlite")).unwrap();
    let (store, _) = oxy_userdata::PersonStore::load(directory.path().join("people.json")).unwrap();
    store_faces_at(
        &library,
        "/photos/a.jpg",
        &[("obs-1", rect(0.1, 0.1, 0.2, 0.2))],
    );
    store_faces_at(
        &library,
        "/photos/b.jpg",
        &[("obs-2", rect(0.1, 0.1, 0.2, 0.2))],
    );
    store
        .upsert_person(PersonRecord {
            person_id: "alice".into(),
            display_name: "Alice".into(),
            linked_tag_id: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    for observation in ["obs-1", "obs-2"] {
        store
            .set_decision(DecisionRecord {
                observation_id: Some(observation.into()),
                asset_id: "asset".into(),
                asset_path: PathBuf::from(if observation == "obs-1" {
                    "/photos/a.jpg"
                } else {
                    "/photos/b.jpg"
                }),
                region: rect(0.1, 0.1, 0.2, 0.2),
                decision: FaceDecision::ConfirmPerson {
                    person_id: "alice".into(),
                },
                created_at_ms: 1,
                proposed_similarity: None,
            })
            .unwrap();
    }
    library
        .replace_user_data(&store.persons(), &store.decisions())
        .unwrap();

    // The user deletes one photo.
    store.remove_decisions(Path::new("/photos/a.jpg")).unwrap();
    library
        .remove_asset_face_state(Path::new("/photos/a.jpg"))
        .unwrap();
    library
        .replace_user_data(&store.persons(), &store.decisions())
        .unwrap();

    assert_eq!(store.decisions().len(), 1);
    assert!(
        library
            .face_observations_for_asset(Path::new("/photos/a.jpg"))
            .unwrap()
            .is_empty(),
        "the deleted asset's cache is gone"
    );
    assert_eq!(
        library.persons().unwrap()[0].face_count,
        1,
        "Alice keeps the photo that still exists"
    );
}
