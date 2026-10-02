use super::*;

fn payload<T: Serialize>(kind: &str, input: &T) -> Result<String, PeopleError> {
    Ok(serde_json::to_string(&(kind, input))?)
}
fn replay<T: serde::de::DeserializeOwned>(
    c: &Connection,
    id: &str,
    payload: &str,
) -> Result<Option<T>, PeopleError> {
    if id.trim().is_empty() {
        return Err(PeopleError::PersonConflict);
    }
    db::event(c, id)?
        .map(|(original, result)| {
            if original != payload {
                return Err(PeopleError::PersonConflict);
            }
            Ok(serde_json::from_str(&result)?)
        })
        .transpose()
}
fn record<T: Serialize>(
    c: &Connection,
    id: &str,
    payload: &str,
    result: &T,
) -> Result<(), PeopleError> {
    db::record_event(c, id, payload, &serde_json::to_string(result)?)?;
    Ok(())
}

impl People {
    pub fn set_global_person_tag(&self, input: &SetGlobalPersonTag) -> Result<(), PeopleError> {
        let payload = payload("globalTag", input)?;
        let mut c = self.store.write();
        let tx = c.transaction().map_err(StoreError::from)?;
        if let Some(result) = replay(&tx, &input.request_id, &payload)? {
            return Ok(result);
        }
        if !db::catalog(&tx)?
            .iter()
            .any(|p| p.id == input.person_id && p.revision == input.expected_revision)
        {
            return Err(PeopleError::PersonConflict);
        }
        if let Some(tag) = input.tag_id
            && !repo::tags::tag_exists(&tx, tag)?
        {
            return Err(PeopleError::MissingTagParent);
        }
        db::set_tag(&tx, &input.person_id, input.tag_id)?;
        db::bump(&tx, &input.person_id)?;
        repo::cross::reconcile_global_person_sources(&tx, &input.person_id)?;
        record(&tx, &input.request_id, &payload, &())?;
        tx.commit().map_err(StoreError::from)?;
        Ok(())
    }
    pub fn save_person_tuple_geometry(
        &self,
        input: &SavePersonTupleGeometry,
        source_identity: &str,
    ) -> Result<PersonTuple, PeopleError> {
        if (input.face_box.is_none() && input.body_box.is_none())
            || !crate::geometry::valid_box(input.face_box)
            || !crate::geometry::valid_box(input.body_box)
            || input.asset_path.parent() != Some(input.folder_path.as_path())
        {
            return Err(PeopleError::InvalidPersonInstance);
        }
        let payload = payload("geometry", input)?;
        let mut c = self.store.write();
        let tx = c.transaction().map_err(StoreError::from)?;
        if let Some(result) =
            replay::<(PersonTuple, Option<PersonTuple>)>(&tx, &input.request_id, &payload)?
        {
            return Ok(result.0);
        }
        let before = if let Some(id) = &input.instance_id {
            let w = workspace(&tx, &input.folder_path)?;
            let old = w
                .groups
                .into_iter()
                .flat_map(|g| g.members)
                .find(|t| t.id == *id && t.asset_path == input.asset_path)
                .ok_or(PeopleError::MissingPersonRecord)?;
            if old.revision != input.expected_revision {
                return Err(PeopleError::PersonConflict);
            }
            Some(old)
        } else {
            if input.expected_revision != 0 {
                return Err(PeopleError::PersonConflict);
            }
            None
        };
        let id = before
            .as_ref()
            .map(|t| t.id.clone())
            .unwrap_or(repo::people::new_id(&tx)?);
        let face = input
            .face_box
            .map(|b| serde_json::to_string(&b))
            .transpose()?;
        let body = input
            .body_box
            .map(|b| serde_json::to_string(&b))
            .transpose()?;
        if before.as_ref().is_some_and(|t| t.revision > 0) {
            repo::people::update_instance_geometry(
                &tx,
                &id,
                face.as_deref(),
                body.as_deref(),
                &input.source_revision,
                Some(source_identity),
            )?;
        } else {
            repo::people::insert_instance(
                &tx,
                &repo::people::NewInstance {
                    id: &id,
                    folder_path: &input.folder_path.to_string_lossy(),
                    asset_path: &input.asset_path.to_string_lossy(),
                    source_revision: &input.source_revision,
                    source_identity_revision: Some(source_identity),
                    face_box_json: face.as_deref(),
                    body_box_json: body.as_deref(),
                },
            )?;
        }
        for r in db::reviews(&tx, Some(&input.folder_path.to_string_lossy()))?
            .iter()
            .filter(|r| r.instance_id == id)
        {
            if r.decision != PersonReviewDecision::DoesNotBelong {
                db::review(&tx, &id, &r.person_id, PersonReviewDecision::Pending)?;
            }
            db::remove_reference(&tx, &r.person_id, &id)?;
            db::bump(&tx, &r.person_id)?;
            repo::cross::reconcile_global_person_sources(&tx, &r.person_id)?;
        }
        let tuple = db::tuples(&tx, Some(&input.folder_path.to_string_lossy()))?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or(PeopleError::MissingPersonRecord)?;
        record(&tx, &input.request_id, &payload, &(tuple.clone(), before))?;
        tx.commit().map_err(StoreError::from)?;
        Ok(tuple)
    }

    pub fn save_global_person(
        &self,
        input: &SaveGlobalPerson,
    ) -> Result<GlobalPerson, PeopleError> {
        let name = input.display_name.trim();
        if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            return Err(failure("请输入有效的人物姓名"));
        }
        let payload = payload("savePerson", input)?;
        self.initialize_global_people()?;
        let mut c = self.store.write();
        let tx = c.transaction().map_err(StoreError::from)?;
        if let Some(result) = replay(&tx, &input.request_id, &payload)? {
            return Ok(result);
        }
        let id = if let Some(id) = &input.id {
            if db::rename(&tx, id, name, input.expected_revision)? != 1 {
                return Err(PeopleError::PersonConflict);
            }
            id.clone()
        } else {
            if input.expected_revision != 0 {
                return Err(PeopleError::PersonConflict);
            }
            let id = repo::people::new_id(&tx)?;
            db::insert_person(&tx, &id, name)?;
            id
        };
        let result = db::catalog(&tx)?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or(PeopleError::MissingPersonRecord)?;
        record(&tx, &input.request_id, &payload, &result)?;
        tx.commit().map_err(StoreError::from)?;
        Ok(result)
    }

    pub fn review_person_tuples(&self, input: &ReviewPersonTuples) -> Result<usize, PeopleError> {
        let payload = payload("reviewTuples", input)?;
        if let Some(result) = replay(&self.store.read(), &input.request_id, &payload)? {
            return Ok(result);
        }
        if input.tuples.is_empty() || input.tuples.len() > 5000 {
            return Err(failure("请选择 1–5000 个人物实例"));
        }
        let folder = input.folder_path.canonicalize()?;
        let mut seen = HashSet::new();
        for t in &input.tuples {
            if !seen.insert(&t.id)
                || t.needs_review
                || t.asset_path.parent() != Some(folder.as_path())
                || oxy_fs::observe_file(&t.asset_path)?.revision_id() != t.source_identity_revision
            {
                return Err(failure("实例或源图已变化，请刷新后重新核对"));
            }
        }
        let mut c = self.store.write();
        let tx = c.transaction().map_err(StoreError::from)?;
        if let Some(result) = replay(&tx, &input.request_id, &payload)? {
            return Ok(result);
        }
        let current = workspace(&tx, &folder)?;
        if current.revision != input.workspace_revision {
            return Err(PeopleError::PersonConflict);
        }
        if !db::catalog(&tx)?.iter().any(|p| p.id == input.person_id) {
            return Err(PeopleError::MissingPersonRecord);
        }
        let manual = db::tuples(&tx, Some(&folder.to_string_lossy()))?;
        let reviews = db::reviews(&tx, Some(&folder.to_string_lossy()))?;
        let mut affected = HashSet::from([input.person_id.clone()]);
        let mut changed = 0;
        for tuple in &input.tuples {
            if !current
                .groups
                .iter()
                .flat_map(|g| &g.members)
                .any(|t| t == tuple)
            {
                return Err(PeopleError::PersonConflict);
            }
            // A transfer must not erase an explicit rejection or another decision.
            if input.decision == PersonReviewDecision::Pending
                && reviews
                    .iter()
                    .any(|r| r.instance_id == tuple.id && r.person_id == input.person_id)
            {
                continue;
            }
            let others: Vec<_> = reviews
                .iter()
                .filter(|r| {
                    r.instance_id == tuple.id
                        && r.person_id != input.person_id
                        && r.decision == PersonReviewDecision::Belongs
                })
                .collect();
            if input.decision != PersonReviewDecision::DoesNotBelong
                && !others.is_empty()
                && !input.replace_confirmed
            {
                return Err(failure(
                    "选中实例已有其他已确认人物；请明确选择“替换已有归属”后再指定",
                ));
            }
            if input.decision != PersonReviewDecision::DoesNotBelong && input.replace_confirmed {
                for other in others {
                    affected.insert(other.person_id.clone());
                    db::review(
                        &tx,
                        &tuple.id,
                        &other.person_id,
                        PersonReviewDecision::DoesNotBelong,
                    )?;
                    db::remove_reference(&tx, &other.person_id, &tuple.id)?;
                    db::bump(&tx, &other.person_id)?;
                }
            }
            // Bulk transfer to pending cannot undo an explicit previous decision.
            if !manual.iter().any(|t| t.id == tuple.id) {
                repo::people::insert_instance(
                    &tx,
                    &repo::people::NewInstance {
                        id: &tuple.id,
                        folder_path: &folder.to_string_lossy(),
                        asset_path: &tuple.asset_path.to_string_lossy(),
                        source_revision: &tuple.source_revision,
                        source_identity_revision: Some(&tuple.source_identity_revision),
                        face_box_json: tuple
                            .face_box
                            .map(|b| serde_json::to_string(&b))
                            .transpose()?
                            .as_deref(),
                        body_box_json: tuple
                            .body_box
                            .map(|b| serde_json::to_string(&b))
                            .transpose()?
                            .as_deref(),
                    },
                )?;
            }
            db::review(&tx, &tuple.id, &input.person_id, input.decision)?;
            if input.decision != PersonReviewDecision::DoesNotBelong
                || tuple
                    .person_id
                    .as_ref()
                    .is_none_or(|id| *id == input.person_id)
            {
                db::target(&tx, &tuple.id, &input.person_id)?;
            }
            if input.decision != PersonReviewDecision::Belongs {
                db::remove_reference(&tx, &input.person_id, &tuple.id)?;
            }
            changed += 1;
        }
        db::bump(&tx, &input.person_id)?;
        for id in affected {
            repo::cross::reconcile_global_person_sources(&tx, &id)?;
        }
        record(&tx, &input.request_id, &payload, &changed)?;
        tx.commit().map_err(StoreError::from)?;
        Ok(changed)
    }

    pub fn set_global_person_reference(
        &self,
        input: &SetGlobalPersonReference,
    ) -> Result<(), PeopleError> {
        let payload = payload("reference", input)?;
        if let Some(result) = replay(&self.store.read(), &input.request_id, &payload)? {
            return Ok(result);
        }
        let tuple = db::tuples(&self.store.read(), None)?
            .into_iter()
            .find(|t| t.id == input.instance_id)
            .ok_or(PeopleError::MissingPersonRecord)?;
        if input.enabled
            && (tuple.needs_review
                || tuple.face_box.is_none() && tuple.body_box.is_none()
                || oxy_fs::observe_file(&tuple.asset_path)?.revision_id()
                    != tuple.source_identity_revision)
        {
            return Err(failure("参考实例已失效，请重新核对"));
        }
        let mut c = self.store.write();
        let tx = c.transaction().map_err(StoreError::from)?;
        if let Some(result) = replay(&tx, &input.request_id, &payload)? {
            return Ok(result);
        }
        let p = db::catalog(&tx)?
            .into_iter()
            .find(|p| p.id == input.person_id)
            .ok_or(PeopleError::MissingPersonRecord)?;
        if p.revision != input.expected_revision {
            return Err(PeopleError::PersonConflict);
        }
        if input.enabled {
            if !db::reviews(&tx, None)?.iter().any(|r| {
                r.instance_id == input.instance_id
                    && r.person_id == input.person_id
                    && r.decision == PersonReviewDecision::Belongs
            }) {
                return Err(failure("请先确认该实例属于此人"));
            }
            if db::tuples(&tx, None)?
                .into_iter()
                .find(|t| t.id == input.instance_id)
                .as_ref()
                != Some(&tuple)
            {
                return Err(PeopleError::PersonConflict);
            }
            db::add_reference(&tx, &input.person_id, &input.instance_id)?;
        } else {
            db::remove_reference(&tx, &input.person_id, &input.instance_id)?;
        }
        db::bump(&tx, &input.person_id)?;
        record(&tx, &input.request_id, &payload, &())?;
        tx.commit().map_err(StoreError::from)?;
        Ok(())
    }
}
