use super::requests::{payload, record, replay};
use super::*;

impl People {
    pub fn save_person_tuple_geometry(
        &self,
        input: &SavePersonTupleGeometry,
        source_identity: &str,
    ) -> Result<PersonTuple, PeopleError> {
        if (input.face_box.is_none() && input.body_box.is_none())
            || !crate::people::geometry::valid_box(input.face_box)
            || !crate::people::geometry::valid_box(input.body_box)
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
            .unwrap_or(crate::people::repo::manual::new_id(&tx)?);
        let face = input
            .face_box
            .map(|b| serde_json::to_string(&b))
            .transpose()?;
        let body = input
            .body_box
            .map(|b| serde_json::to_string(&b))
            .transpose()?;
        if before.as_ref().is_some_and(|t| t.revision > 0) {
            crate::people::repo::manual::update_instance_geometry(
                &tx,
                &id,
                face.as_deref(),
                body.as_deref(),
                &input.source_revision,
                Some(source_identity),
            )?;
        } else {
            crate::people::repo::manual::insert_instance(
                &tx,
                &crate::people::repo::manual::NewInstance {
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
            crate::app::workflows::person_tags::reconcile_global_person_sources(&tx, &r.person_id)?;
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
            let id = crate::people::repo::manual::new_id(&tx)?;
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
                || crate::files::observe_file(&tuple.asset_path)?.revision_id()
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
