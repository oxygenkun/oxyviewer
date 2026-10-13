//! Recovery statements; workflow owns transactions and recovery policy.
use crate::storage::{Connection, StoreError};

pub(crate) struct RecoveryRow {
    pub id: i64,
    pub operation: String,
    pub phase: String,
    pub proofs: Option<String>,
    pub detail: Option<String>,
}

pub(crate) fn prepare(connection: &Connection, operation: &str) -> Result<i64, StoreError> {
    connection.execute(
        "INSERT INTO file_operation_recovery(operation_json,phase) VALUES(?1,'prepared')",
        [operation],
    )?;
    Ok(connection.last_insert_rowid())
}

pub(crate) fn applied(connection: &Connection, id: i64, proofs: &str) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE file_operation_recovery SET phase='filesApplied',proofs_json=?2 WHERE id=?1",
        rusqlite::params![id, proofs],
    )?;
    Ok(())
}

pub(crate) fn failure(connection: &Connection, id: i64, detail: &str) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE file_operation_recovery SET detail=?2 WHERE id=?1",
        rusqlite::params![id, detail],
    )?;
    Ok(())
}

pub(crate) fn complete(connection: &Connection, id: i64) -> Result<(), StoreError> {
    connection.execute("DELETE FROM file_operation_recovery WHERE id=?1", [id])?;
    Ok(())
}

pub(crate) fn pending(connection: &Connection) -> Result<Vec<RecoveryRow>, StoreError> {
    let mut statement = connection.prepare("SELECT id,operation_json,phase,proofs_json,detail FROM file_operation_recovery ORDER BY id")?;
    Ok(statement
        .query_map([], |row| {
            Ok(RecoveryRow {
                id: row.get("id")?,
                operation: row.get("operation_json")?,
                phase: row.get("phase")?,
                proofs: row.get("proofs_json")?,
                detail: row.get("detail")?,
            })
        })?
        .collect::<Result<_, _>>()?)
}
