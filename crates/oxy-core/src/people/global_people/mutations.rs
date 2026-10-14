use super::requests::{payload, record, replay};
use super::*;

impl People {
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
