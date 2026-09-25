//! Person identity, instances, reviews, references, and their history.
//!
//! Every table here holds something the user named, confirmed, or rejected, so
//! no cache clear reaches it. The statements that write a revision, keep an
//! event, and record a request id live here; the rules about which of them a
//! given user action performs live in the crate that owns the person domain.

use crate::StoreError;
use oxy_domain::{
    FolderPerson, HistoricalPerson, ManualPersonAnchor, PersonInstance, PersonReview,
    PersonReviewDecision, PersonTagLink, PersonTagOverride,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

/// The columns that make up a [`FolderPerson`], including the two correlated
/// lookups that report its confirmed reference and its pending review count.
const FOLDER_PERSON_COLUMNS: &str = "id,folder_path,display_name,identity_confirmed,revision,
     (SELECT instance_id FROM person_references WHERE subject_id=folder_people.id LIMIT 1) AS reference_instance_id,
     (SELECT count(*) FROM person_review_decisions WHERE subject_id=folder_people.id AND decision='pending') AS pending_count";

/// One manual instance of a folder, as the person filter reads it.
///
/// The filter decides staleness from the asset it was given, so the record
/// stops at the stored columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceRecord {
    pub asset_path: String,
    pub source_revision: String,
    pub needs_review: bool,
    pub decision: Option<String>,
}

/// The result of a user action that was already applied, keyed by request id.
///
/// Every mutation takes a request id so a retried command replays instead of
/// applying twice.
pub fn request_result(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<(String, String)>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT operation,entity_id FROM person_request_results WHERE request_id=?1",
            [request_id],
            |row| Ok((row.get("operation")?, row.get("entity_id")?)),
        )
        .optional()?)
}

/// Records that a request id was applied to one entity.
pub fn record_request_result(
    connection: &Connection,
    request_id: &str,
    operation: &str,
    entity_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_request_results VALUES (?1,?2,?3)",
        params![request_id, operation, entity_id],
    )?;
    Ok(())
}

/// The subject a recorded history event belongs to.
pub fn history_event_subject(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<String>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT subject_id FROM person_history_events WHERE request_id=?1",
            [request_id],
            |row| row.get("subject_id"),
        )
        .optional()?)
}

/// Every historical identity, by display name.
pub fn list_historical_people(
    connection: &Connection,
) -> Result<Vec<HistoricalPerson>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT id,display_name,reference_asset_path,reference_source_revision,revision
         FROM historical_people ORDER BY display_name,id",
    )?;
    Ok(statement
        .query_map([], row_historical_person)?
        .collect::<Result<Vec<_>, _>>()?)
}

/// One historical identity by id.
pub fn historical_person(
    connection: &Connection,
    id: &str,
) -> Result<Option<HistoricalPerson>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT id,display_name,reference_asset_path,reference_source_revision,revision
             FROM historical_people WHERE id=?1",
            [id],
            row_historical_person,
        )
        .optional()?)
}

/// Whether an identity with this id exists.
pub fn historical_person_exists(connection: &Connection, id: &str) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM historical_people WHERE id=?1) AS present",
        [id],
        |row| row.get("present"),
    )?)
}

/// Creates an identity from the reference the folder person was confirmed on.
pub fn insert_historical_person(
    connection: &Connection,
    id: &str,
    display_name: &str,
    reference_asset_path: &str,
    reference_source_revision: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO historical_people(id,display_name,reference_asset_path,reference_source_revision)
         VALUES (?1,?2,?3,?4)",
        params![
            id,
            display_name,
            reference_asset_path,
            reference_source_revision
        ],
    )?;
    Ok(())
}

/// The identity a folder subject is linked to, looked up through its folder.
pub fn historical_link(
    connection: &Connection,
    folder_path: &str,
    subject_id: &str,
) -> Result<Option<String>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT l.historical_person_id FROM folder_historical_links l
             JOIN folder_people f ON f.id=l.subject_id
             WHERE f.folder_path=?1 AND f.id=?2",
            params![folder_path, subject_id],
            |row| row.get("historical_person_id"),
        )
        .optional()?)
}

/// The same link, but only while the folder person is still at `revision`.
pub fn historical_link_at_revision(
    connection: &Connection,
    folder_path: &str,
    subject_id: &str,
    revision: i64,
) -> Result<Option<String>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT l.historical_person_id FROM folder_historical_links l
             JOIN folder_people f ON f.id=l.subject_id
             WHERE f.folder_path=?1 AND f.id=?2 AND f.revision=?3",
            params![folder_path, subject_id, revision],
            |row| row.get("historical_person_id"),
        )
        .optional()?)
}

/// The identity this subject is linked to, without the folder check.
pub fn historical_link_of_subject(
    connection: &Connection,
    subject_id: &str,
) -> Result<Option<String>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT historical_person_id FROM folder_historical_links WHERE subject_id=?1",
            [subject_id],
            |row| row.get("historical_person_id"),
        )
        .optional()?)
}

/// Links a subject to an identity, replacing any previous link.
pub fn upsert_historical_link(
    connection: &Connection,
    subject_id: &str,
    historical_person_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO folder_historical_links(subject_id,historical_person_id) VALUES (?1,?2)
         ON CONFLICT(subject_id) DO UPDATE SET historical_person_id=excluded.historical_person_id,
         linked_at=unixepoch()",
        params![subject_id, historical_person_id],
    )?;
    Ok(())
}

/// Removes a subject's link to a historical identity.
pub fn delete_historical_link(connection: &Connection, subject_id: &str) -> Result<(), StoreError> {
    connection.execute(
        "DELETE FROM folder_historical_links WHERE subject_id=?1",
        [subject_id],
    )?;
    Ok(())
}

/// Turns every current link into an `unlink` event, before the link goes away.
pub fn carry_links_into_events(
    connection: &Connection,
    subject_id: &str,
    request_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_history_events(subject_id,historical_person_id,event_kind,request_id)
         SELECT subject_id,historical_person_id,'unlink',?2 FROM folder_historical_links WHERE subject_id=?1",
        params![subject_id, request_id],
    )?;
    Ok(())
}

/// Records one link or unlink of a folder subject to an identity.
pub fn insert_history_event(
    connection: &Connection,
    subject_id: &str,
    historical_person_id: &str,
    event_kind: &str,
    request_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_history_events(subject_id,historical_person_id,event_kind,request_id) VALUES (?1,?2,?3,?4)",
        params![subject_id, historical_person_id, event_kind, request_id],
    )?;
    Ok(())
}

/// The confirmed reference a subject is allowed to link from.
///
/// Returns its display name, the asset path of the reference instance, and the
/// revision that instance was captured at.
pub fn link_source(
    connection: &Connection,
    subject_id: &str,
    folder_path: &str,
    expected_revision: i64,
) -> Result<Option<(String, String, String)>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT f.display_name,i.asset_path,i.source_revision FROM folder_people f
             JOIN person_references r ON r.subject_id=f.id
             JOIN person_manual_instances i ON i.id=r.instance_id
             JOIN person_review_decisions d ON d.instance_id=i.id AND d.subject_id=f.id
             WHERE f.id=?1 AND f.folder_path=?2 AND f.revision=?3
               AND f.identity_confirmed=1 AND d.decision='belongs'
               AND i.needs_review=0 AND i.face_box IS NOT NULL
               AND r.source_revision=i.source_revision",
            params![subject_id, folder_path, expected_revision],
            |row| {
                Ok((
                    row.get("display_name")?,
                    row.get("asset_path")?,
                    row.get("source_revision")?,
                ))
            },
        )
        .optional()?)
}

/// A fresh 128-bit identifier, hex encoded.
pub fn new_id(connection: &Connection) -> Result<String, StoreError> {
    Ok(connection.query_row("SELECT lower(hex(randomblob(16))) AS id", [], |row| {
        row.get("id")
    })?)
}

/// One folder person by id, with its reference and pending count.
pub fn folder_person(
    connection: &Connection,
    id: &str,
    folder_path: &str,
) -> Result<Option<FolderPerson>, StoreError> {
    let sql = format!("SELECT {FOLDER_PERSON_COLUMNS} FROM folder_people WHERE id=?1 AND folder_path=?2");
    Ok(connection
        .query_row(&sql, params![id, folder_path], row_person)
        .optional()?)
}

/// Every folder person in this folder, in creation order.
pub fn list_folder_people(
    connection: &Connection,
    folder_path: &str,
) -> Result<Vec<FolderPerson>, StoreError> {
    let sql = format!(
        "SELECT {FOLDER_PERSON_COLUMNS} FROM folder_people WHERE folder_path=?1 ORDER BY created_at,id"
    );
    let mut statement = connection.prepare(&sql)?;
    Ok(statement
        .query_map([folder_path], row_person)?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Creates an unnamed, unconfirmed folder person.
pub fn insert_folder_person(
    connection: &Connection,
    id: &str,
    folder_path: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO folder_people(id,folder_path) VALUES (?1,?2)",
        params![id, folder_path],
    )?;
    Ok(())
}

/// Confirms the identity of a folder person and names it.
///
/// Returns how many rows changed, so a caller can tell a successful confirm
/// from one that raced with another edit.
pub fn confirm_identity(
    connection: &Connection,
    subject_id: &str,
    folder_path: &str,
    display_name: &str,
    expected_revision: i64,
) -> Result<usize, StoreError> {
    Ok(connection.execute(
        "UPDATE folder_people SET display_name=?1,identity_confirmed=1,revision=revision+1,
         updated_at=unixepoch() WHERE id=?2 AND folder_path=?3 AND revision=?4",
        params![display_name, subject_id, folder_path, expected_revision],
    )?)
}

/// Clears a folder person back to an unnamed, unconfirmed state.
pub fn reset_identity(
    connection: &Connection,
    subject_id: &str,
    folder_path: &str,
    display_name: &str,
    expected_revision: i64,
) -> Result<usize, StoreError> {
    Ok(connection.execute(
        "UPDATE folder_people SET display_name=?1,identity_confirmed=0,revision=revision+1,updated_at=unixepoch() WHERE id=?2 AND folder_path=?3 AND revision=?4",
        params![display_name, subject_id, folder_path, expected_revision],
    )?)
}

/// Bumps a folder person's revision, marking its identity as changed.
pub fn bump_subject_revision(
    connection: &Connection,
    subject_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE folder_people SET revision=revision+1,updated_at=unixepoch() WHERE id=?1",
        [subject_id],
    )?;
    Ok(())
}

/// Bumps the revision of every subject that references one instance.
pub fn bump_referencing_subjects(
    connection: &Connection,
    instance_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE folder_people SET revision=revision+1 WHERE id IN (SELECT subject_id FROM person_references WHERE instance_id=?1)",
        [instance_id],
    )?;
    Ok(())
}

/// Records which instance a subject's identity was confirmed from.
pub fn replace_reference(
    connection: &Connection,
    subject_id: &str,
    instance_id: &str,
    source_revision: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT OR REPLACE INTO person_references(subject_id,instance_id,source_revision)
         VALUES (?1,?2,?3)",
        params![subject_id, instance_id, source_revision],
    )?;
    Ok(())
}

/// Drops every reference held by one subject.
pub fn delete_references_of_subject(
    connection: &Connection,
    subject_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "DELETE FROM person_references WHERE subject_id=?1",
        [subject_id],
    )?;
    Ok(())
}

/// Drops every reference to one instance.
pub fn delete_references_of_instance(
    connection: &Connection,
    instance_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "DELETE FROM person_references WHERE instance_id=?1",
        [instance_id],
    )?;
    Ok(())
}

/// Drops one subject's reference to one instance.
pub fn delete_reference(
    connection: &Connection,
    subject_id: &str,
    instance_id: &str,
) -> Result<usize, StoreError> {
    Ok(connection.execute(
        "DELETE FROM person_references WHERE subject_id=?1 AND instance_id=?2",
        params![subject_id, instance_id],
    )?)
}

/// The source revision a confirmed reference was captured at.
pub fn reference_source_revision(
    connection: &Connection,
    instance_id: &str,
    folder_path: &str,
    subject_id: &str,
) -> Result<Option<String>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT i.source_revision FROM person_manual_instances i
         JOIN person_review_decisions r ON r.instance_id=i.id
         WHERE i.id=?1 AND i.folder_path=?2 AND r.subject_id=?3
           AND r.decision='belongs' AND i.face_box IS NOT NULL AND i.needs_review=0",
            params![instance_id, folder_path, subject_id],
            |row| row.get("source_revision"),
        )
        .optional()?)
}

/// Records one identity change: confirm, reset, or a rename.
pub fn insert_identity_event(
    connection: &Connection,
    subject_id: &str,
    event_kind: &str,
    display_name: &str,
    revision: i64,
    request_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_identity_events(subject_id,event_kind,display_name,revision,request_id) VALUES (?1,?2,?3,?4,?5)",
        params![subject_id, event_kind, display_name, revision, request_id],
    )?;
    Ok(())
}

/// One manual instance by id, scoped to its folder.
pub fn instance(
    connection: &Connection,
    id: &str,
    folder_path: &str,
) -> Result<Option<PersonInstance>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT id,folder_path,asset_path,source_revision,face_box,body_box,needs_review,revision
             FROM person_manual_instances WHERE id=?1 AND folder_path=?2",
            params![id, folder_path],
            row_instance,
        )
        .optional()?)
}

/// Every manual instance over one asset, in creation order.
pub fn list_instances(
    connection: &Connection,
    folder_path: &str,
    asset_path: &str,
) -> Result<Vec<PersonInstance>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT id,folder_path,asset_path,source_revision,face_box,body_box,needs_review,revision
         FROM person_manual_instances WHERE folder_path=?1 AND asset_path=?2 ORDER BY created_at,id",
    )?;
    Ok(statement
        .query_map(params![folder_path, asset_path], row_instance)?
        .collect::<Result<Vec<_>, _>>()?)
}

/// The alignment evidence of every manual instance over one asset.
pub fn list_anchors(
    connection: &Connection,
    folder_path: &str,
    asset_path: &str,
) -> Result<Vec<ManualPersonAnchor>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT id,source_identity_revision,face_box,body_box,needs_review,revision
         FROM person_manual_instances WHERE folder_path=?1 AND asset_path=?2 ORDER BY id",
    )?;
    let rows = statement.query_map(params![folder_path, asset_path], |row| {
        let face: Option<String> = row.get("face_box")?;
        let body: Option<String> = row.get("body_box")?;
        let parse = |value: Option<String>| {
            value
                .map(|value| serde_json::from_str::<[f64; 4]>(&value))
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
        };
        Ok(ManualPersonAnchor {
            instance_id: row.get("id")?,
            source_identity_revision: row.get("source_identity_revision")?,
            face_box: parse(face)?,
            body_box: parse(body)?,
            needs_review: row.get::<_, i64>("needs_review")? != 0,
            revision: row.get("revision")?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// The stored columns of a manual instance that is about to be created.
///
/// The boxes arrive already encoded because their text form is a row detail,
/// and a caller that never has a box never has to build one.
pub struct NewInstance<'a> {
    pub id: &'a str,
    pub folder_path: &'a str,
    pub asset_path: &'a str,
    pub source_revision: &'a str,
    pub source_identity_revision: Option<&'a str>,
    pub face_box_json: Option<&'a str>,
    pub body_box_json: Option<&'a str>,
}

/// Creates a manual instance.
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

/// The source identity a manual instance was captured from, if it has one.
pub fn source_identity_revision(
    connection: &Connection,
    instance_id: &str,
) -> Result<Option<String>, StoreError> {
    Ok(connection.query_row(
        "SELECT source_identity_revision FROM person_manual_instances WHERE id=?1",
        [instance_id],
        |row| row.get("source_identity_revision"),
    )?)
}

/// The asset path one instance belongs to.
pub fn instance_asset_path(
    connection: &Connection,
    instance_id: &str,
) -> Result<String, StoreError> {
    Ok(connection.query_row(
        "SELECT asset_path FROM person_manual_instances WHERE id=?1",
        [instance_id],
        |row| row.get("asset_path"),
    )?)
}

/// Records the geometry an instance had before an edit.
pub fn insert_instance_event(
    connection: &Connection,
    instance_id: &str,
    previous_json: &str,
    request_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_instance_events(instance_id,previous_json,request_id) VALUES (?1,?2,?3)",
        params![instance_id, previous_json, request_id],
    )?;
    Ok(())
}

/// Writes new geometry onto an instance and clears its review flag.
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

/// Keeps the previous decisions in the audit while requiring them again.
pub fn carry_review_decisions_into_events(
    connection: &Connection,
    instance_id: &str,
    request_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_review_events(instance_id,subject_id,decision,revision,request_id) SELECT instance_id,subject_id,CASE WHEN decision='doesNotBelong' THEN decision ELSE 'pending' END,revision+1,?2 || ':' || subject_id FROM person_review_decisions WHERE instance_id=?1",
        params![instance_id, request_id],
    )?;
    Ok(())
}

/// Resets the decisions on a changed instance, keeping every rejection.
pub fn require_review_after_change(
    connection: &Connection,
    instance_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE person_review_decisions SET decision=CASE WHEN decision='doesNotBelong' THEN decision ELSE 'pending' END,revision=revision+1 WHERE instance_id=?1",
        [instance_id],
    )?;
    Ok(())
}

/// The subjects that hold a decision about one instance.
pub fn subjects_of_instance(
    connection: &Connection,
    instance_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut statement =
        connection.prepare("SELECT subject_id FROM person_review_decisions WHERE instance_id=?1")?;
    Ok(statement
        .query_map([instance_id], |row| row.get::<_, String>("subject_id"))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Whether this subject may hold a decision about this instance.
pub fn review_allowed(
    connection: &Connection,
    instance_id: &str,
    subject_id: &str,
    folder_path: &str,
) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM person_manual_instances i JOIN folder_people p
           ON p.folder_path=i.folder_path WHERE i.id=?1 AND p.id=?2 AND i.folder_path=?3) AS present",
        params![instance_id, subject_id, folder_path],
        |row| row.get("present"),
    )?)
}

/// The current revision of one review decision.
pub fn review_revision(
    connection: &Connection,
    instance_id: &str,
    subject_id: &str,
) -> Result<Option<i64>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT revision FROM person_review_decisions WHERE instance_id=?1 AND subject_id=?2",
            params![instance_id, subject_id],
            |row| row.get("revision"),
        )
        .optional()?)
}

/// Writes a decision, replacing the previous one at its new revision.
pub fn upsert_review_decision(
    connection: &Connection,
    instance_id: &str,
    subject_id: &str,
    decision: PersonReviewDecision,
    revision: i64,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_review_decisions(instance_id,subject_id,decision,revision)
         VALUES (?1,?2,?3,?4)
         ON CONFLICT(instance_id,subject_id) DO UPDATE SET decision=excluded.decision,
           revision=excluded.revision,updated_at=unixepoch()",
        params![instance_id, subject_id, decision.as_str(), revision],
    )?;
    Ok(())
}

/// Records one decision change in the review audit.
pub fn insert_review_event(
    connection: &Connection,
    instance_id: &str,
    subject_id: &str,
    decision: PersonReviewDecision,
    revision: i64,
    request_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_review_events(instance_id,subject_id,decision,revision,request_id)
         VALUES (?1,?2,?3,?4,?5)",
        params![
            instance_id,
            subject_id,
            decision.as_str(),
            revision,
            request_id
        ],
    )?;
    Ok(())
}

/// One review, joined back to the instance it is about.
pub fn review(
    connection: &Connection,
    instance_id: &str,
    subject_id: &str,
) -> Result<Option<(PersonInstance, PersonReviewDecision, i64)>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT i.id,i.folder_path,i.asset_path,i.source_revision,i.face_box,i.body_box,i.needs_review,i.revision,
                    r.decision AS decision,r.revision AS review_revision
             FROM person_review_decisions r
             JOIN person_manual_instances i ON i.id=r.instance_id
             WHERE r.instance_id=?1 AND r.subject_id=?2",
            params![instance_id, subject_id],
            |row| {
                Ok((
                    row_instance(row)?,
                    PersonReviewDecision::from_text(&row.get::<_, String>("decision")?),
                    row.get::<_, i64>("review_revision")?,
                ))
            },
        )
        .optional()?)
}

/// Every review one subject holds in a folder, in asset order.
pub fn list_reviews(
    connection: &Connection,
    folder_path: &str,
    subject_id: &str,
) -> Result<Vec<PersonReview>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT i.id,i.folder_path,i.asset_path,i.source_revision,i.face_box,i.body_box,i.needs_review,i.revision,
                COALESCE(r.decision,'pending') AS decision,
                COALESCE(r.revision,0) AS review_revision
         FROM person_manual_instances i
         JOIN person_review_decisions r ON r.instance_id=i.id AND r.subject_id=?2
         WHERE i.folder_path=?1 ORDER BY i.asset_path,i.id",
    )?;
    Ok(statement
        .query_map(params![folder_path, subject_id], |row| {
            Ok(PersonReview {
                instance: row_instance(row)?,
                subject_id: subject_id.to_owned(),
                decision: PersonReviewDecision::from_text(&row.get::<_, String>("decision")?),
                revision: row.get("review_revision")?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Every manual instance of a folder, with this subject's decision on it.
///
/// The person filter reads the whole folder and then filters the page it was
/// given, so the query returns a raw record per instance rather than a
/// decision about an asset.
pub fn instance_records(
    connection: &Connection,
    folder_path: &str,
    subject_id: Option<&str>,
) -> Result<Vec<InstanceRecord>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT i.asset_path,i.source_revision,i.needs_review,r.decision
         FROM person_manual_instances i
         LEFT JOIN person_review_decisions r ON r.instance_id=i.id AND r.subject_id=?2
         WHERE i.folder_path=?1",
    )?;
    Ok(statement
        .query_map(params![folder_path, subject_id], |row| {
            Ok(InstanceRecord {
                asset_path: row.get("asset_path")?,
                source_revision: row.get("source_revision")?,
                needs_review: row.get::<_, bool>("needs_review")?,
                decision: row.get("decision")?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

/// The tag bound to a historical identity, through the folder subject.
///
/// `enabled` is only reported when there is a tag to apply, so a disabled link
/// with no tag reads the same as no link at all.
pub fn person_tag_link(
    connection: &Connection,
    folder_path: &Path,
    subject_id: &str,
) -> Result<Option<PersonTagLink>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT t.historical_person_id,t.tag_id,t.enabled,t.revision
             FROM folder_people f JOIN folder_historical_links l ON l.subject_id=f.id
             LEFT JOIN person_tag_links t ON t.historical_person_id=l.historical_person_id
             WHERE f.folder_path=?1 AND f.id=?2 AND t.historical_person_id IS NOT NULL",
            params![folder_path.to_string_lossy(), subject_id],
            row_person_tag_link,
        )
        .optional()?)
}

/// The tag bound to one historical identity.
pub fn person_tag_link_of(
    connection: &Connection,
    historical_person_id: &str,
) -> Result<Option<PersonTagLink>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT historical_person_id,tag_id,enabled,revision FROM person_tag_links
             WHERE historical_person_id=?1",
            [historical_person_id],
            row_person_tag_link,
        )
        .optional()?)
}

/// The current revision of one person-to-tag link.
pub fn person_tag_link_revision(
    connection: &Connection,
    historical_person_id: &str,
) -> Result<Option<i64>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT revision FROM person_tag_links WHERE historical_person_id=?1",
            [historical_person_id],
            |row| row.get("revision"),
        )
        .optional()?)
}

/// Binds a tag to a historical identity, advancing its revision.
pub fn upsert_person_tag_link(
    connection: &Connection,
    historical_person_id: &str,
    tag_id: Option<i64>,
    enabled: bool,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_tag_links(historical_person_id,tag_id,enabled,revision)
         VALUES (?1,?2,?3,1)
         ON CONFLICT(historical_person_id) DO UPDATE SET
           tag_id=excluded.tag_id,enabled=excluded.enabled,
           revision=person_tag_links.revision+1,updated_at=unixepoch()",
        params![historical_person_id, tag_id, enabled],
    )?;
    Ok(())
}

/// The folder subjects linked to one historical identity.
pub fn subjects_of_historical_person(
    connection: &Connection,
    historical_person_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT subject_id FROM folder_historical_links WHERE historical_person_id=?1",
    )?;
    Ok(statement
        .query_map([historical_person_id], |row| {
            row.get::<_, String>("subject_id")
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

/// The subject a folder person maps to through its historical link.
pub fn historical_person_of_subject(
    connection: &Connection,
    folder_path: &Path,
    subject_id: &str,
) -> Result<Option<String>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT l.historical_person_id FROM folder_people f
             JOIN folder_historical_links l ON l.subject_id=f.id
             WHERE f.folder_path=?1 AND f.id=?2",
            params![folder_path.to_string_lossy(), subject_id],
            |row| row.get("historical_person_id"),
        )
        .optional()?)
}

/// One per-asset suppression of a person's tag.
pub fn person_tag_override(
    connection: &Connection,
    folder_path: &Path,
    subject_id: &str,
    asset_path: &Path,
) -> Result<Option<PersonTagOverride>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT o.historical_person_id,o.asset_path,o.suppressed,o.revision
             FROM folder_people f JOIN folder_historical_links l ON l.subject_id=f.id
             JOIN person_tag_overrides o ON o.historical_person_id=l.historical_person_id
             WHERE f.folder_path=?1 AND f.id=?2 AND o.asset_path=?3",
            params![
                folder_path.to_string_lossy(),
                subject_id,
                asset_path.to_string_lossy()
            ],
            row_person_tag_override,
        )
        .optional()?)
}

/// The current revision of one per-asset suppression.
pub fn person_tag_override_revision(
    connection: &Connection,
    historical_person_id: &str,
    asset_path: &str,
) -> Result<Option<i64>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT revision FROM person_tag_overrides WHERE historical_person_id=?1 AND asset_path=?2",
            params![historical_person_id, asset_path],
            |row| row.get("revision"),
        )
        .optional()?)
}

/// Writes a per-asset suppression, advancing its revision.
pub fn upsert_person_tag_override(
    connection: &Connection,
    historical_person_id: &str,
    asset_path: &str,
    suppressed: bool,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO person_tag_overrides(historical_person_id,asset_path,suppressed,revision)
         VALUES (?1,?2,?3,1)
         ON CONFLICT(historical_person_id,asset_path) DO UPDATE SET
           suppressed=excluded.suppressed,revision=person_tag_overrides.revision+1",
        params![historical_person_id, asset_path, suppressed],
    )?;
    Ok(())
}

/// One per-asset suppression after a write, keyed by both columns.
pub fn person_tag_override_of(
    connection: &Connection,
    historical_person_id: &str,
    asset_path: &str,
) -> Result<Option<PersonTagOverride>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT historical_person_id,asset_path,suppressed,revision FROM person_tag_overrides
             WHERE historical_person_id=?1 AND asset_path=?2",
            params![historical_person_id, asset_path],
            row_person_tag_override,
        )
        .optional()?)
}

/// Points every row that names one asset at another path.
///
/// Only the same-folder move uses this: the photo is the same photo, so its
/// instances, its historical references, and its suppressions follow it.
pub fn repoint_asset_rows(
    connection: &Connection,
    source: &str,
    destination: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE person_manual_instances SET asset_path=?2 WHERE asset_path=?1",
        params![source, destination],
    )?;
    connection.execute(
        "UPDATE historical_people SET reference_asset_path=?2 WHERE reference_asset_path=?1",
        params![source, destination],
    )?;
    connection.execute(
        "UPDATE person_tag_overrides SET asset_path=?2 WHERE asset_path=?1",
        params![source, destination],
    )?;
    Ok(())
}

/// Flags every instance over an asset that left its folder for review.
pub fn require_review_for_asset(
    connection: &Connection,
    asset_path: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE person_manual_instances SET needs_review=1 WHERE asset_path=?1",
        params![asset_path],
    )?;
    Ok(())
}

/// The tag a person identity pushes onto one asset, from the link, the
/// per-asset overrides, and the review decision recorded for it.
///
/// This is the one query that reads both domains: it starts from the person's
/// link and ends at the tag the assignment tables should carry.
pub fn person_source_tag(
    connection: &Connection,
    subject_id: &str,
    asset_path: &str,
) -> Result<Option<i64>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT t.tag_id FROM folder_historical_links l
         JOIN person_tag_links t ON t.historical_person_id=l.historical_person_id
         WHERE l.subject_id=?1 AND t.enabled=1 AND t.tag_id IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM person_tag_overrides o
             WHERE o.historical_person_id=l.historical_person_id
               AND o.asset_path=?2 AND o.suppressed=1)
           AND EXISTS (SELECT 1 FROM person_review_decisions r
             JOIN person_manual_instances i ON i.id=r.instance_id
             WHERE r.subject_id=?1 AND i.asset_path=?2
               AND r.decision='belongs' AND i.needs_review=0)",
            params![subject_id, asset_path],
            |row| row.get("tag_id"),
        )
        .optional()?)
}

fn row_person(row: &rusqlite::Row<'_>) -> rusqlite::Result<FolderPerson> {
    Ok(FolderPerson {
        id: row.get("id")?,
        folder_path: PathBuf::from(row.get::<_, String>("folder_path")?),
        display_name: row.get("display_name")?,
        identity_confirmed: row.get::<_, i64>("identity_confirmed")? != 0,
        revision: row.get("revision")?,
        reference_instance_id: row.get("reference_instance_id")?,
        pending_count: row.get("pending_count")?,
    })
}

fn row_instance(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersonInstance> {
    let face: Option<String> = row.get("face_box")?;
    let body: Option<String> = row.get("body_box")?;
    Ok(PersonInstance {
        id: row.get("id")?,
        folder_path: PathBuf::from(row.get::<_, String>("folder_path")?),
        asset_path: PathBuf::from(row.get::<_, String>("asset_path")?),
        source_revision: row.get("source_revision")?,
        face_box: face.and_then(|value| serde_json::from_str(&value).ok()),
        body_box: body.and_then(|value| serde_json::from_str(&value).ok()),
        needs_review: row.get::<_, i64>("needs_review")? != 0,
        revision: row.get("revision")?,
    })
}

fn row_historical_person(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoricalPerson> {
    Ok(HistoricalPerson {
        id: row.get("id")?,
        display_name: row.get("display_name")?,
        reference_asset_path: PathBuf::from(row.get::<_, String>("reference_asset_path")?),
        reference_source_revision: row.get("reference_source_revision")?,
        revision: row.get("revision")?,
    })
}

fn row_person_tag_link(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersonTagLink> {
    Ok(PersonTagLink {
        historical_person_id: row.get("historical_person_id")?,
        tag_id: row.get("tag_id")?,
        enabled: row.get::<_, bool>("enabled")?
            && row.get::<_, Option<i64>>("tag_id")?.is_some(),
        revision: row.get("revision")?,
    })
}

fn row_person_tag_override(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersonTagOverride> {
    Ok(PersonTagOverride {
        historical_person_id: row.get("historical_person_id")?,
        asset_path: row.get::<_, String>("asset_path")?.into(),
        suppressed: row.get("suppressed")?,
        revision: row.get("revision")?,
    })
}

