use super::*;
use oxy_runtime::CancellationToken;

#[test]
fn persisted_edge_faces_can_be_loaded_confirmed_and_edited() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().canonicalize().unwrap();
    let people = crate::testing::in_memory();
    crate::clusters::tests::analyzed_fixture(&people, &folder);
    let initial = people
        .run_people_grouping(
            &RunPeopleGrouping {
                folder_path: folder.clone(),
                person_ids: None,
                similarity: 0.4,
                request_id: "edge-group".into(),
            },
            &CancellationToken::default(),
        )
        .unwrap();
    // Actual persisted right-edge HIF geometry: JSON round trips moved the
    // right edge one ulp outside the image. Exercise the existing-cache path.
    let face = [
        0.9391934204101564,
        0.5245018684651769,
        0.06080657958984375,
        0.1437004968845185,
    ];
    assert!(face[0] + face[2] > 1.0);
    {
        let c = people.store.write();
        let mut saved = load(&c, &folder).unwrap().unwrap();
        for tuple in saved.groups.iter_mut().flat_map(|g| &mut g.members) {
            tuple.face_box = Some(face);
        }
        db::save_suggestions(
            &c,
            &folder.to_string_lossy(),
            &saved.run_id,
            &serde_json::to_string(&saved).unwrap(),
        )
        .unwrap();
    }
    let loaded = people.folder_people_workspace(&folder).unwrap();
    assert_eq!(loaded.unknown_count, initial.unknown_count);
    let tuple = &loaded.groups[0].members[0];
    let identity = person(&people, "Edge face");
    people
        .review_person_tuples(&request(
            &people,
            &folder,
            &tuple.id,
            &identity,
            PersonReviewDecision::Belongs,
            "confirm-edge",
        ))
        .unwrap();
    let confirmed = people.folder_people_workspace(&folder).unwrap();
    let tuple = confirmed
        .groups
        .iter()
        .flat_map(|g| &g.members)
        .find(|t| t.person_id.as_deref() == Some(&identity.id))
        .unwrap();
    assert_eq!(tuple.decision, Some(PersonReviewDecision::Belongs));
    people
        .save_person_tuple_geometry(
            &SavePersonTupleGeometry {
                folder_path: folder.clone(),
                asset_path: tuple.asset_path.clone(),
                instance_id: Some(tuple.id.clone()),
                expected_revision: tuple.revision,
                source_revision: tuple.source_revision.clone(),
                face_box: tuple.face_box,
                body_box: Some([0.5, 0.5, 0.5, 0.5]),
                request_id: "edit-edge".into(),
            },
            &tuple.source_identity_revision,
        )
        .unwrap();
    assert!(people.folder_people_workspace(&folder).is_ok());
}

fn person(people: &People, name: &str) -> GlobalPerson {
    people
        .save_global_person(&SaveGlobalPerson {
            id: None,
            display_name: name.into(),
            expected_revision: 0,
            request_id: format!("create:{name}"),
        })
        .unwrap()
}
fn manual(
    people: &People,
    folder: &Path,
    name: &str,
    face: Option<[f64; 4]>,
    body: Option<[f64; 4]>,
) -> PersonTuple {
    let path = folder.join("multi.jpg");
    if !path.exists() {
        std::fs::write(&path, b"source").unwrap();
    }
    let asset = oxy_fs::FsCatalog::default().get_asset(&path).unwrap();
    people
        .save_person_tuple_geometry(
            &SavePersonTupleGeometry {
                folder_path: folder.into(),
                asset_path: path.clone(),
                instance_id: None,
                expected_revision: 0,
                source_revision: format!("{}:{}", asset.size_bytes, asset.modified_at_ms),
                face_box: face,
                body_box: body,
                request_id: name.into(),
            },
            &oxy_fs::observe_file(&path).unwrap().revision_id(),
        )
        .unwrap()
}
fn request(
    people: &People,
    folder: &Path,
    id: &str,
    person: &GlobalPerson,
    decision: PersonReviewDecision,
    request_id: &str,
) -> ReviewPersonTuples {
    let workspace = people.folder_people_workspace(folder).unwrap();
    let tuple = workspace
        .groups
        .iter()
        .flat_map(|g| &g.members)
        .find(|t| t.id == id)
        .unwrap()
        .clone();
    ReviewPersonTuples {
        folder_path: folder.into(),
        workspace_revision: workspace.revision,
        tuples: vec![tuple],
        person_id: person.id.clone(),
        decision,
        replace_confirmed: false,
        request_id: request_id.into(),
    }
}

#[test]
fn identities_are_global_and_two_people_in_one_photo_remain_independent() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first = root.join("one");
    let second = root.join("two");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let people = crate::testing::open(&root.join("people.sqlite"));
    let a = person(&people, "甲");
    let b = person(&people, "乙");
    let face = Some([0.1, 0.1, 0.2, 0.2]);
    let body = Some([0.1, 0.1, 0.3, 0.8]);
    let left = manual(&people, &first, "left", face, body);
    let right = manual(&people, &first, "right", None, Some([0.5, 0.1, 0.3, 0.8]));
    let other = manual(&people, &second, "other", face, None);
    people
        .review_person_tuples(&request(
            &people,
            &first,
            &left.id,
            &a,
            PersonReviewDecision::Belongs,
            "left-a",
        ))
        .unwrap();
    people
        .review_person_tuples(&request(
            &people,
            &first,
            &right.id,
            &b,
            PersonReviewDecision::Pending,
            "right-b",
        ))
        .unwrap();
    people
        .review_person_tuples(&request(
            &people,
            &second,
            &other.id,
            &a,
            PersonReviewDecision::Pending,
            "other-a",
        ))
        .unwrap();
    let w = people.folder_people_workspace(&first).unwrap();
    assert_eq!(w.known_count, 2);
    assert_eq!(w.unknown_count, 0);
    assert_eq!(
        w.groups
            .iter()
            .flat_map(|g| &g.members)
            .find(|t| t.id == left.id)
            .unwrap()
            .body_box,
        body
    );
    assert_eq!(
        w.groups
            .iter()
            .flat_map(|g| &g.members)
            .find(|t| t.id == right.id)
            .unwrap()
            .face_box,
        None
    );
    let current = people
        .global_people()
        .unwrap()
        .into_iter()
        .find(|p| p.id == a.id)
        .unwrap();
    people
        .save_global_person(&SaveGlobalPerson {
            id: Some(a.id.clone()),
            display_name: "新姓名".into(),
            expected_revision: current.revision,
            request_id: "rename".into(),
        })
        .unwrap();
    let reopened = crate::testing::open(&root.join("people.sqlite"));
    assert_eq!(
        reopened
            .global_people()
            .unwrap()
            .iter()
            .find(|p| p.id == a.id)
            .unwrap()
            .display_name,
        "新姓名"
    );
    assert_eq!(
        reopened.folder_people_workspace(&second).unwrap().groups[0].person_id,
        Some(a.id)
    );
    people.store.clear_rebuildable_cache().unwrap();
    assert_eq!(
        people.folder_people_workspace(&first).unwrap().known_count,
        2
    );
}

#[test]
fn batch_is_atomic_preserves_negatives_and_requires_explicit_reassignment() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().canonicalize().unwrap();
    let people = crate::testing::in_memory();
    let a = person(&people, "甲");
    let b = person(&people, "乙");
    let one = manual(&people, &folder, "one", Some([0.1, 0.1, 0.2, 0.2]), None);
    let two = manual(&people, &folder, "two", None, Some([0.5, 0.1, 0.2, 0.8]));
    people
        .review_person_tuples(&request(
            &people,
            &folder,
            &one.id,
            &a,
            PersonReviewDecision::Belongs,
            "belongs",
        ))
        .unwrap();
    let w = people.folder_people_workspace(&folder).unwrap();
    let mut batch = request(
        &people,
        &folder,
        &two.id,
        &b,
        PersonReviewDecision::Pending,
        "batch",
    );
    batch.tuples.push(
        w.groups
            .iter()
            .flat_map(|g| &g.members)
            .find(|t| t.id == one.id)
            .unwrap()
            .clone(),
    );
    assert!(people.review_person_tuples(&batch).is_err());
    assert!(
        !db::reviews(&people.store.read(), None)
            .unwrap()
            .iter()
            .any(|r| r.person_id == b.id)
    );
    batch.replace_confirmed = true;
    assert_eq!(people.review_person_tuples(&batch).unwrap(), 2);
    assert_eq!(people.review_person_tuples(&batch).unwrap(), 2);
    let rejected = request(
        &people,
        &folder,
        &one.id,
        &a,
        PersonReviewDecision::Pending,
        "preserve-negative",
    );
    assert_eq!(people.review_person_tuples(&rejected).unwrap(), 0);
    assert!(
        db::reviews(&people.store.read(), None)
            .unwrap()
            .iter()
            .any(|r| r.instance_id == one.id
                && r.person_id == a.id
                && r.decision == PersonReviewDecision::DoesNotBelong)
    );
    let stale = request(
        &people,
        &folder,
        &two.id,
        &b,
        PersonReviewDecision::Belongs,
        "stale",
    );
    std::fs::write(&two.asset_path, b"changed source content").unwrap();
    assert!(people.review_person_tuples(&stale).is_err());
}

#[test]
fn retrieval_uses_only_confirmed_explicit_references_and_preserves_rejections() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let one = root.join("one");
    let two = root.join("two");
    std::fs::create_dir(&one).unwrap();
    std::fs::create_dir(&two).unwrap();
    let people = crate::testing::in_memory();
    crate::clusters::tests::analyzed_fixture(&people, &one);
    crate::clusters::tests::analyzed_fixture(&people, &two);
    let a = person(&people, "甲");
    let run = |folder: &Path, ids: Option<Vec<String>>| RunPeopleGrouping {
        folder_path: folder.into(),
        person_ids: ids,
        similarity: 0.4,
        request_id: "group".into(),
    };
    assert!(
        people
            .run_people_grouping(
                &run(&two, Some(vec![a.id.clone()])),
                &CancellationToken::default()
            )
            .is_err()
    );
    let first = people
        .run_people_grouping(&run(&one, None), &CancellationToken::default())
        .unwrap();
    assert_eq!(first.unknown_count, 1);
    let id = first.groups[0].members[0].id.clone();
    people
        .review_person_tuples(&request(
            &people,
            &one,
            &id,
            &a,
            PersonReviewDecision::Belongs,
            "confirm-ref",
        ))
        .unwrap();
    let a = people
        .global_people()
        .unwrap()
        .into_iter()
        .find(|p| p.id == a.id)
        .unwrap();
    people
        .set_global_person_reference(&SetGlobalPersonReference {
            person_id: a.id.clone(),
            instance_id: id.clone(),
            enabled: true,
            expected_revision: a.revision,
            request_id: "reference".into(),
        })
        .unwrap();
    // JSON round trips in real HIF detections differ by a few ulps. A unique
    // corresponding face must retain its reference vector after persistence.
    {
        let c = people.store.write();
        let tuple = db::tuples(&c, None)
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap();
        let mut face = tuple.face_box.unwrap();
        face[1] += 1e-17;
        repo::people::update_instance_geometry(
            &c,
            &id,
            Some(&serde_json::to_string(&face).unwrap()),
            None,
            &tuple.source_revision,
            Some(&tuple.source_identity_revision),
        )
        .unwrap();
    }
    let found = people
        .run_people_grouping(
            &run(&two, Some(vec![a.id.clone()])),
            &CancellationToken::default(),
        )
        .unwrap();
    assert_eq!(found.known_count, 1);
    assert_eq!(found.unknown_count, 0);
    assert_eq!(found.groups[0].members.len(), 2);
    assert!(
        db::tuples(&people.store.read(), Some(&two.to_string_lossy()))
            .unwrap()
            .is_empty()
    );
    let rejected = found.groups[0].members[0].id.clone();
    people
        .review_person_tuples(&request(
            &people,
            &two,
            &rejected,
            &a,
            PersonReviewDecision::DoesNotBelong,
            "reject",
        ))
        .unwrap();
    let repeated = people
        .run_people_grouping(
            &run(&two, Some(vec![a.id.clone()])),
            &CancellationToken::default(),
        )
        .unwrap();
    let known = repeated
        .groups
        .iter()
        .find(|g| g.person_id == Some(a.id.clone()))
        .unwrap();
    assert_eq!(
        known
            .members
            .iter()
            .filter(|t| t.decision == Some(PersonReviewDecision::Pending))
            .count(),
        1
    );
    let token = CancellationToken::default();
    token.cancel();
    assert!(
        people
            .run_people_grouping(&run(&two, None), &token)
            .is_err()
    );
    assert_eq!(
        people.folder_people_workspace(&two).unwrap().revision,
        repeated.revision
    );
    std::fs::write(
        first.groups[0].members[0].asset_path.clone(),
        b"changed reference",
    )
    .unwrap();
    assert!(
        people
            .run_people_grouping(&run(&two, Some(vec![a.id])), &CancellationToken::default())
            .is_err()
    );
}

#[test]
fn migration_merges_explicit_history_links_but_not_equal_names() {
    let people = crate::testing::in_memory();
    {
        let c = people.store.write();
        repo::people::insert_historical_person(&c, "history", "姓名", "/reference.jpg", "1:1")
            .unwrap();
        for (id, folder) in [("a", "/a"), ("b", "/b"), ("c", "/c")] {
            repo::people::insert_folder_person(&c, id, folder).unwrap();
            repo::people::reset_identity(&c, id, folder, "姓名", 1).unwrap();
        }
        repo::people::upsert_historical_link(&c, "a", "history").unwrap();
        repo::people::upsert_historical_link(&c, "b", "history").unwrap();
    }
    let catalog = people.global_people().unwrap();
    assert_eq!(catalog.len(), 2);
    assert!(catalog.iter().any(|p| p.id == "history"));
    assert!(catalog.iter().any(|p| p.id == "legacy:c"));
    assert_eq!(people.global_people().unwrap(), catalog);
    assert_eq!(
        repo::people::list_folder_people(&people.store.read(), "/a")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn geometry_change_withdraws_reference_and_person_tag_but_preserves_manual_tag() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().canonicalize().unwrap();
    let people = crate::testing::in_memory();
    let a = person(&people, "甲");
    let t = manual(&people, &folder, "face", Some([0.1, 0.1, 0.2, 0.2]), None);
    let tag = {
        let c = people.store.write();
        let tag = repo::tags::insert_tag(&c, None, "人物", "人物", 0).unwrap();
        repo::tags::insert_source(&c, &t.asset_path.to_string_lossy(), tag, "manual", "").unwrap();
        repo::tags::reconcile_effective(&c, &t.asset_path.to_string_lossy(), tag).unwrap();
        tag
    };
    people
        .set_global_person_tag(&SetGlobalPersonTag {
            person_id: a.id.clone(),
            tag_id: Some(tag),
            expected_revision: a.revision,
            request_id: "tag".into(),
        })
        .unwrap();
    people
        .review_person_tuples(&request(
            &people,
            &folder,
            &t.id,
            &a,
            PersonReviewDecision::Belongs,
            "confirm",
        ))
        .unwrap();
    let a = people
        .global_people()
        .unwrap()
        .into_iter()
        .find(|p| p.id == a.id)
        .unwrap();
    people
        .set_global_person_reference(&SetGlobalPersonReference {
            person_id: a.id.clone(),
            instance_id: t.id.clone(),
            enabled: true,
            expected_revision: a.revision,
            request_id: "ref".into(),
        })
        .unwrap();
    assert!(
        !repo::tags::source_tag_ids_of(
            &people.store.read(),
            &t.asset_path.to_string_lossy(),
            "person",
            &format!("global:{}", a.id)
        )
        .unwrap()
        .is_empty()
    );
    let edited = people
        .save_person_tuple_geometry(
            &SavePersonTupleGeometry {
                folder_path: folder,
                asset_path: t.asset_path.clone(),
                instance_id: Some(t.id),
                expected_revision: t.revision,
                source_revision: t.source_revision,
                face_box: t.face_box,
                body_box: Some([0.1, 0.1, 0.3, 0.8]),
                request_id: "edit".into(),
            },
            &t.source_identity_revision,
        )
        .unwrap();
    assert_eq!(edited.decision, Some(PersonReviewDecision::Pending));
    assert!(
        people
            .global_people()
            .unwrap()
            .iter()
            .find(|p| p.id == a.id)
            .unwrap()
            .reference_instance_ids
            .is_empty()
    );
    assert!(
        repo::tags::source_tag_ids_of(
            &people.store.read(),
            &edited.asset_path.to_string_lossy(),
            "person",
            &format!("global:{}", a.id)
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        repo::tags::effective_assignment_present(
            &people.store.read(),
            &edited.asset_path.to_string_lossy(),
            tag
        )
        .unwrap()
    );
}

#[test]
fn conflicting_legacy_links_keep_exclusions_and_original_decisions() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().canonicalize().unwrap();
    let people = crate::testing::in_memory();
    let tuple = manual(&people, &folder, "legacy", Some([0.1, 0.1, 0.2, 0.2]), None);
    {
        let c = people.store.write();
        repo::people::insert_historical_person(
            &c,
            "history",
            "姓名",
            &tuple.asset_path.to_string_lossy(),
            &tuple.source_revision,
        )
        .unwrap();
        for (id, decision) in [
            ("a", PersonReviewDecision::Belongs),
            ("b", PersonReviewDecision::DoesNotBelong),
        ] {
            repo::people::insert_folder_person(&c, id, &folder.to_string_lossy()).unwrap();
            repo::people::upsert_historical_link(&c, id, "history").unwrap();
            repo::people::upsert_review_decision(&c, &tuple.id, id, decision, 1).unwrap();
        }
    }
    let w = people.folder_people_workspace(&folder).unwrap();
    assert_eq!(people.global_people().unwrap().len(), 1);
    assert_eq!(w.known_count, 0);
    assert_eq!(w.unknown_count, 1);
    assert!(
        w.groups
            .iter()
            .any(|g| g.person_id.as_deref() == Some("history")
                && g.members[0].decision == Some(PersonReviewDecision::DoesNotBelong))
    );
    assert_eq!(
        repo::people::list_reviews(&people.store.read(), &folder.to_string_lossy(), "a").unwrap()
            [0]
        .decision,
        PersonReviewDecision::Belongs
    );
    assert_eq!(
        people.folder_people_workspace(&folder).unwrap().revision,
        w.revision
    );
}

#[test]
fn global_gallery_pages_confirmed_history_across_folders_and_preserves_offline_records() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let people = crate::testing::open(&root.join("people.sqlite"));
    let a = person(&people, "Gallery person");
    let b = person(&people, "Other person");
    let mut confirmed_ids = Vec::new();
    for (index, decision) in [
        PersonReviewDecision::Belongs,
        PersonReviewDecision::Belongs,
        PersonReviewDecision::Pending,
        PersonReviewDecision::DoesNotBelong,
        PersonReviewDecision::Deferred,
    ]
    .into_iter()
    .enumerate()
    {
        let folder = root.join(format!("folder-{index}"));
        std::fs::create_dir(&folder).unwrap();
        let tuple = manual(
            &people,
            &folder,
            &format!("manual-{index}"),
            None,
            Some([0.1, 0.1, 0.4, 0.8]),
        );
        people
            .review_person_tuples(&request(
                &people,
                &folder,
                &tuple.id,
                &a,
                decision,
                &format!("review-{index}"),
            ))
            .unwrap();
        if decision == PersonReviewDecision::Belongs {
            confirmed_ids.push(tuple.id);
        }
    }
    let first = people.global_person_gallery(&a.id, 0, 1).unwrap();
    assert_eq!(first.total, 2);
    assert_eq!(first.folder_count, 2);
    assert_eq!(first.tuples.len(), 1);
    assert_eq!(first.tuples[0].id, confirmed_ids[0]);
    assert!(first.tuples[0].body_box.is_some());
    let second = people.global_person_gallery(&a.id, 1, 1).unwrap();
    assert_eq!(second.tuples[0].id, confirmed_ids[1]);
    assert!(
        people
            .global_person_gallery(&a.id, 2, 1)
            .unwrap()
            .tuples
            .is_empty()
    );
    assert_eq!(people.global_person_gallery(&b.id, 0, 24).unwrap().total, 0);
    std::fs::remove_file(&first.tuples[0].asset_path).unwrap();
    assert_eq!(people.global_person_gallery(&a.id, 0, 24).unwrap().total, 2);
    // Group before pagination; multiple confirmed instances in one photo count
    // as one photo, and an offline source remains in its original folder.
    let folder = root.join("folder-0");
    let extra = manual(
        &people,
        &folder,
        "extra-instance",
        Some([0.6, 0.1, 0.2, 0.2]),
        None,
    );
    people
        .review_person_tuples(&request(
            &people,
            &folder,
            &extra.id,
            &a,
            PersonReviewDecision::Belongs,
            "extra-confirm",
        ))
        .unwrap();
    std::fs::remove_file(&extra.asset_path).unwrap();
    let folders = people.global_person_folders(&a.id, 0, 1).unwrap();
    assert_eq!(folders.total, 2);
    assert_eq!(folders.folders.len(), 1);
    assert_eq!(folders.folders[0].folder_path, folder);
    assert_eq!(
        folders.folders[0].cover_asset_path,
        first.tuples[0].asset_path
    );
    assert_eq!(folders.folders[0].photo_count, 1);
    assert_eq!(folders.folders[0].instance_count, 2);
    let second_folder = people.global_person_folders(&a.id, 1, 1).unwrap();
    assert_eq!(second_folder.folders[0].folder_path, root.join("folder-1"));
    assert_eq!(
        second_folder.folders[0].cover_asset_path,
        second.tuples[0].asset_path
    );
    assert!(
        people
            .global_person_folders(&a.id, 2, 1)
            .unwrap()
            .folders
            .is_empty()
    );
    assert_eq!(people.global_person_folders(&b.id, 0, 24).unwrap().total, 0);
    assert_eq!(
        people
            .global_person_folders(&a.id, 0, 0)
            .unwrap()
            .folders
            .len(),
        1
    );
    assert_eq!(
        people
            .global_person_gallery(&a.id, 0, 0)
            .unwrap()
            .tuples
            .len(),
        1
    );
}
