//! Tuple geometry rules applied inside the application's transaction.
use super::*;

pub(crate) struct GeometryOutcome {
    pub tuple: PersonTuple,
    pub previous: Option<PersonTuple>,
    pub affected_people: Vec<String>,
}

pub(crate) fn validate(input: &SavePersonTupleGeometry) -> Result<(), PeopleError> {
    if (input.face_box.is_none() && input.body_box.is_none())
        || !crate::people::geometry::valid_box(input.face_box)
        || !crate::people::geometry::valid_box(input.body_box)
        || input.asset_path.parent() != Some(input.folder_path.as_path())
    {
        return Err(PeopleError::InvalidPersonInstance);
    }
    Ok(())
}

pub(crate) fn apply(
    connection: &Connection,
    input: &SavePersonTupleGeometry,
    source_identity: &str,
) -> Result<GeometryOutcome, PeopleError> {
    let before = if let Some(id) = &input.instance_id {
        let w = workspace(connection, &input.folder_path)?;
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
        .unwrap_or(crate::people::repo::manual::new_id(connection)?);
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
            connection,
            &id,
            face.as_deref(),
            body.as_deref(),
            &input.source_revision,
            Some(source_identity),
        )?;
    } else {
        crate::people::repo::manual::insert_instance(
            connection,
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
    let mut affected_people = Vec::new();
    for r in db::reviews(connection, Some(&input.folder_path.to_string_lossy()))?
        .iter()
        .filter(|r| r.instance_id == id)
    {
        if r.decision != PersonReviewDecision::DoesNotBelong {
            db::review(connection, &id, &r.person_id, PersonReviewDecision::Pending)?;
        }
        db::remove_reference(connection, &r.person_id, &id)?;
        db::bump(connection, &r.person_id)?;
        affected_people.push(r.person_id.clone());
    }
    let tuple = db::tuples(connection, Some(&input.folder_path.to_string_lossy()))?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or(PeopleError::MissingPersonRecord)?;
    Ok(GeometryOutcome {
        tuple,
        previous: before,
        affected_people,
    })
}
