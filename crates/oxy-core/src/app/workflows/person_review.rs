//! Commit reviewed person facts, tag contributions and request result together.
use crate::people::{
    PeopleError,
    global_people::{
        requests,
        review::{self, PreparedReview},
    },
    types::ReviewPersonTuples,
};
use crate::storage::{Store, StoreError};

pub(crate) fn review_person_tuples(
    store: &Store,
    input: &ReviewPersonTuples,
) -> Result<usize, PeopleError> {
    let (payload, folder) = match review::prepare(store, input)? {
        PreparedReview::Replayed(result) => return Ok(result),
        PreparedReview::Pending { payload, folder } => (payload, folder),
    };
    let mut connection = store.write();
    let transaction = connection.transaction().map_err(StoreError::from)?;
    if let Some(result) = requests::replay(&transaction, &input.request_id, &payload)? {
        return Ok(result);
    }
    let outcome = review::apply(&transaction, &folder, input)?;
    for person in outcome.affected_people {
        super::person_tags::reconcile_global_person_sources(&transaction, &person)?;
    }
    requests::record(&transaction, &input.request_id, &payload, &outcome.changed)?;
    transaction.commit().map_err(StoreError::from)?;
    Ok(outcome.changed)
}
