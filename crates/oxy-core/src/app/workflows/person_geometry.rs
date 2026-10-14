//! Atomically edit tuple geometry, withdraw references and update tag contributions.
use crate::people::{
    PeopleError,
    global_people::{geometry, requests},
    types::{PersonTuple, SavePersonTupleGeometry},
};
use crate::storage::{Store, StoreError};

pub(crate) fn save_person_tuple_geometry(
    store: &Store,
    input: &SavePersonTupleGeometry,
    source_identity: &str,
) -> Result<PersonTuple, PeopleError> {
    geometry::validate(input)?;
    let payload = requests::payload("geometry", input)?;
    let mut connection = store.write();
    let transaction = connection.transaction().map_err(StoreError::from)?;
    if let Some((tuple, _)) = requests::replay::<(PersonTuple, Option<PersonTuple>)>(
        &transaction,
        &input.request_id,
        &payload,
    )? {
        return Ok(tuple);
    }
    let outcome = geometry::apply(&transaction, input, source_identity)?;
    for person in outcome.affected_people {
        super::person_tags::reconcile_global_person_sources(&transaction, &person)?;
    }
    requests::record(
        &transaction,
        &input.request_id,
        &payload,
        &(&outcome.tuple, &outcome.previous),
    )?;
    transaction.commit().map_err(StoreError::from)?;
    Ok(outcome.tuple)
}
