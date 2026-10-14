//! Idempotent request payloads and results owned by the global person feature.
use crate::people::{PeopleError, repo::global as db};
use crate::storage::Connection;
use serde::{Serialize, de::DeserializeOwned};

pub(crate) fn payload<T: Serialize>(kind: &str, input: &T) -> Result<String, PeopleError> {
    Ok(serde_json::to_string(&(kind, input))?)
}

pub(crate) fn replay<T: DeserializeOwned>(
    connection: &Connection,
    id: &str,
    payload: &str,
) -> Result<Option<T>, PeopleError> {
    if id.trim().is_empty() {
        return Err(PeopleError::PersonConflict);
    }
    db::event(connection, id)?
        .map(|(original, result)| {
            if original != payload {
                return Err(PeopleError::PersonConflict);
            }
            Ok(serde_json::from_str(&result)?)
        })
        .transpose()
}

pub(crate) fn record<T: Serialize>(
    connection: &Connection,
    id: &str,
    payload: &str,
    result: &T,
) -> Result<(), PeopleError> {
    db::record_event(connection, id, payload, &serde_json::to_string(result)?)?;
    Ok(())
}
