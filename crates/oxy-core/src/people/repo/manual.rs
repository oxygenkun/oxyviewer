//! Statements for user-owned person tuple geometry.
use crate::storage::{Connection, StoreError};
use rusqlite::params;

pub fn new_id(connection: &Connection) -> Result<String, StoreError> {
    Ok(
        connection.query_row("SELECT lower(hex(randomblob(16))) AS id", [], |row| {
            row.get("id")
        })?,
    )
}

pub struct NewInstance<'a> {
    pub id: &'a str,
    pub folder_path: &'a str,
    pub asset_path: &'a str,
    pub source_revision: &'a str,
    pub source_identity_revision: Option<&'a str>,
    pub face_box_json: Option<&'a str>,
    pub body_box_json: Option<&'a str>,
}

pub fn insert_instance(
    connection: &Connection,
    instance: &NewInstance<'_>,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_manual_instances(id,folder_path,asset_path,source_revision,source_identity_revision,face_box,body_box)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            instance.id,
            instance.folder_path,
            instance.asset_path,
            instance.source_revision,
            instance.source_identity_revision,
            instance.face_box_json,
            instance.body_box_json
        ],
    )?;
    Ok(())
}

pub fn update_instance_geometry(
    connection: &Connection,
    instance_id: &str,
    face_box_json: Option<&str>,
    body_box_json: Option<&str>,
    source_revision: &str,
    source_identity_revision: Option<&str>,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE person_manual_instances SET face_box=?1,body_box=?2,source_revision=?3,source_identity_revision=?4,needs_review=0,revision=revision+1,updated_at=unixepoch() WHERE id=?5",
        params![
            face_box_json,
            body_box_json,
            source_revision,
            source_identity_revision,
            instance_id
        ],
    )?;
    Ok(())
}
