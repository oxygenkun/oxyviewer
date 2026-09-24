use crate::{Library, LibraryError};
use oxy_domain::{
    AssetSummary, ConfirmFolderPerson, CreatePersonInstance, FolderPerson, HistoricalPerson,
    LinkHistoricalPerson, PersonFilter, PersonFilterState, PersonInstance, PersonReview,
    PersonReviewDecision, ResetFolderPerson, SetPersonReview, UnlinkHistoricalPerson,
    UpdatePersonInstance,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

/// Tables created by [`ensure_schema`]. User-owned because this module lives in
/// [`crate::user`]; there is no second list that could claim otherwise.
pub(super) const TABLES: &[&str] = &[
    "folder_people",
    "person_manual_instances",
    "person_review_decisions",
    "person_review_events",
    "person_identity_events",
    "person_instance_events",
    "person_references",
    "person_request_results",
    "historical_people",
    "folder_historical_links",
    "person_history_events",
    "person_tag_links",
    "person_tag_overrides",
];

pub(crate) fn ensure_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS folder_people (
           id TEXT PRIMARY KEY,
           folder_path TEXT NOT NULL,
           display_name TEXT,
           identity_confirmed INTEGER NOT NULL DEFAULT 0,
           revision INTEGER NOT NULL DEFAULT 1,
           created_at INTEGER NOT NULL DEFAULT (unixepoch()),
           updated_at INTEGER NOT NULL DEFAULT (unixepoch())
         );
         CREATE INDEX IF NOT EXISTS folder_people_folder ON folder_people(folder_path);
         CREATE TABLE IF NOT EXISTS person_manual_instances (
           id TEXT PRIMARY KEY,
           folder_path TEXT NOT NULL,
           asset_path TEXT NOT NULL,
           source_revision TEXT NOT NULL,
           source_identity_revision TEXT,
           face_box TEXT,
           body_box TEXT,
           needs_review INTEGER NOT NULL DEFAULT 0,
           revision INTEGER NOT NULL DEFAULT 1,
           created_at INTEGER NOT NULL DEFAULT (unixepoch()),
           updated_at INTEGER NOT NULL DEFAULT (unixepoch())
         );
         CREATE INDEX IF NOT EXISTS person_manual_instances_asset
           ON person_manual_instances(folder_path, asset_path);
         CREATE TABLE IF NOT EXISTS person_review_decisions (
           instance_id TEXT NOT NULL REFERENCES person_manual_instances(id),
           subject_id TEXT NOT NULL REFERENCES folder_people(id),
           decision TEXT NOT NULL CHECK(decision IN ('pending','belongs','doesNotBelong','deferred')),
           revision INTEGER NOT NULL DEFAULT 1,
           updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
           PRIMARY KEY(instance_id, subject_id)
         );
         CREATE TABLE IF NOT EXISTS person_review_events (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           instance_id TEXT NOT NULL,
           subject_id TEXT NOT NULL,
           decision TEXT NOT NULL,
           revision INTEGER NOT NULL,
           request_id TEXT NOT NULL UNIQUE,
           changed_at INTEGER NOT NULL DEFAULT (unixepoch())
         );
         CREATE TABLE IF NOT EXISTS person_identity_events (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           subject_id TEXT NOT NULL,
           event_kind TEXT NOT NULL,
           display_name TEXT,
           revision INTEGER NOT NULL,
           request_id TEXT NOT NULL UNIQUE,
           changed_at INTEGER NOT NULL DEFAULT (unixepoch())
         );
         CREATE TABLE IF NOT EXISTS person_references (
           subject_id TEXT NOT NULL REFERENCES folder_people(id),
           instance_id TEXT NOT NULL REFERENCES person_manual_instances(id),
           source_revision TEXT NOT NULL,
           confirmed_at INTEGER NOT NULL DEFAULT (unixepoch()),
           PRIMARY KEY(subject_id, instance_id)
         );
         CREATE TABLE IF NOT EXISTS person_instance_events (
           id INTEGER PRIMARY KEY AUTOINCREMENT, instance_id TEXT NOT NULL,
           previous_json TEXT NOT NULL, request_id TEXT NOT NULL UNIQUE,
           changed_at INTEGER NOT NULL DEFAULT (unixepoch())
         );
         CREATE TABLE IF NOT EXISTS person_request_results (
           request_id TEXT PRIMARY KEY,
           operation TEXT NOT NULL,
           entity_id TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS historical_people (
           id TEXT PRIMARY KEY,
           display_name TEXT NOT NULL,
           reference_asset_path TEXT NOT NULL,
           reference_source_revision TEXT NOT NULL,
           revision INTEGER NOT NULL DEFAULT 1,
           created_at INTEGER NOT NULL DEFAULT (unixepoch())
         );
         CREATE TABLE IF NOT EXISTS folder_historical_links (
           subject_id TEXT PRIMARY KEY REFERENCES folder_people(id),
           historical_person_id TEXT NOT NULL REFERENCES historical_people(id),
           linked_at INTEGER NOT NULL DEFAULT (unixepoch())
         );
         CREATE INDEX IF NOT EXISTS folder_historical_links_person
           ON folder_historical_links(historical_person_id);
         CREATE TABLE IF NOT EXISTS person_history_events (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           subject_id TEXT NOT NULL,
           historical_person_id TEXT NOT NULL,
           event_kind TEXT NOT NULL CHECK(event_kind IN ('link','unlink')),
           request_id TEXT NOT NULL UNIQUE,
           linked_at INTEGER NOT NULL DEFAULT (unixepoch())
         );
         CREATE TABLE IF NOT EXISTS person_tag_links (
           historical_person_id TEXT PRIMARY KEY REFERENCES historical_people(id),
           tag_id INTEGER REFERENCES custom_tags(id) ON DELETE SET NULL,
           enabled INTEGER NOT NULL DEFAULT 0,
           revision INTEGER NOT NULL DEFAULT 1,
           updated_at INTEGER NOT NULL DEFAULT (unixepoch())
         );
         CREATE TABLE IF NOT EXISTS person_tag_overrides (
           historical_person_id TEXT NOT NULL REFERENCES historical_people(id),
           asset_path TEXT NOT NULL,
           suppressed INTEGER NOT NULL DEFAULT 1,
           revision INTEGER NOT NULL DEFAULT 1,
           PRIMARY KEY(historical_person_id,asset_path)
         );",
    )?;
    let has_source_identity: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('person_manual_instances')
           WHERE name='source_identity_revision')",
        [],
        |row| row.get(0),
    )?;
    if !has_source_identity {
        connection.execute(
            "ALTER TABLE person_manual_instances ADD COLUMN source_identity_revision TEXT",
            [],
        )?;
    }
    Ok(())
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn decision_text(value: PersonReviewDecision) -> &'static str {
    match value {
        PersonReviewDecision::Pending => "pending",
        PersonReviewDecision::Belongs => "belongs",
        PersonReviewDecision::DoesNotBelong => "doesNotBelong",
        PersonReviewDecision::Deferred => "deferred",
    }
}

fn parse_decision(value: &str) -> PersonReviewDecision {
    match value {
        "belongs" => PersonReviewDecision::Belongs,
        "doesNotBelong" => PersonReviewDecision::DoesNotBelong,
        "deferred" => PersonReviewDecision::Deferred,
        _ => PersonReviewDecision::Pending,
    }
}

fn valid_box(value: Option<[f64; 4]>) -> bool {
    value.is_none_or(|[x, y, width, height]| {
        [x, y, width, height].iter().all(|part| part.is_finite())
            && x >= 0.0
            && y >= 0.0
            && width > 0.0
            && height > 0.0
            && x + width <= 1.0
            && y + height <= 1.0
    })
}

fn valid_source_identity(value: Option<&str>) -> bool {
    value
        .is_none_or(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn row_person(row: &rusqlite::Row<'_>) -> rusqlite::Result<FolderPerson> {
    Ok(FolderPerson {
        id: row.get(0)?,
        folder_path: PathBuf::from(row.get::<_, String>(1)?),
        display_name: row.get(2)?,
        identity_confirmed: row.get::<_, i64>(3)? != 0,
        revision: row.get(4)?,
        reference_instance_id: row.get(5)?,
        pending_count: row.get(6)?,
    })
}

fn row_instance(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersonInstance> {
    let face: Option<String> = row.get(4)?;
    let body: Option<String> = row.get(5)?;
    Ok(PersonInstance {
        id: row.get(0)?,
        folder_path: PathBuf::from(row.get::<_, String>(1)?),
        asset_path: PathBuf::from(row.get::<_, String>(2)?),
        source_revision: row.get(3)?,
        face_box: face.and_then(|value| serde_json::from_str(&value).ok()),
        body_box: body.and_then(|value| serde_json::from_str(&value).ok()),
        needs_review: row.get::<_, i64>(6)? != 0,
        revision: row.get(7)?,
    })
}

fn row_historical_person(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoricalPerson> {
    Ok(HistoricalPerson {
        id: row.get(0)?,
        display_name: row.get(1)?,
        reference_asset_path: PathBuf::from(row.get::<_, String>(2)?),
        reference_source_revision: row.get(3)?,
        revision: row.get(4)?,
    })
}

/// Internal alignment evidence. A missing full source identity means the
/// instance predates source-identity capture and must not auto-align.
#[derive(Debug, Clone, PartialEq)]
pub struct ManualPersonAnchor {
    pub instance_id: String,
    pub source_identity_revision: Option<String>,
    pub face_box: Option<[f64; 4]>,
    pub body_box: Option<[f64; 4]>,
    pub needs_review: bool,
    pub revision: i64,
}

impl Library {
    pub fn list_historical_people(&self) -> Result<Vec<HistoricalPerson>, LibraryError> {
        let connection = self.read_connection();
        let mut statement = connection.prepare(
            "SELECT id,display_name,reference_asset_path,reference_source_revision,revision
             FROM historical_people ORDER BY display_name,id",
        )?;
        Ok(statement
            .query_map([], row_historical_person)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_historical_link(
        &self,
        folder_path: &Path,
        subject_id: &str,
    ) -> Result<Option<String>, LibraryError> {
        let connection = self.read_connection();
        Ok(connection
            .query_row(
                "SELECT l.historical_person_id FROM folder_historical_links l
                 JOIN folder_people f ON f.id=l.subject_id
                 WHERE f.folder_path=?1 AND f.id=?2",
                params![path_text(folder_path), subject_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn link_historical_person(
        &self,
        input: &LinkHistoricalPerson,
    ) -> Result<HistoricalPerson, LibraryError> {
        if input.request_id.is_empty() {
            return Err(LibraryError::InvalidPersonInstance);
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let replay: Option<(String, String)> = transaction
            .query_row(
                "SELECT operation,entity_id FROM person_request_results WHERE request_id=?1",
                [&input.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let history_id = if let Some((operation, id)) = replay {
            let replay_subject: Option<String> = transaction
                .query_row(
                    "SELECT subject_id FROM person_history_events WHERE request_id=?1",
                    [&input.request_id],
                    |row| row.get(0),
                )
                .optional()?;
            if operation != "linkHistory"
                || replay_subject.as_deref() != Some(input.subject_id.as_str())
                || input
                    .historical_person_id
                    .as_ref()
                    .is_some_and(|expected| expected != &id)
            {
                return Err(LibraryError::PersonConflict);
            }
            id
        } else {
            let source: Option<(String, String, String)> = transaction
                .query_row(
                    "SELECT f.display_name,i.asset_path,i.source_revision FROM folder_people f
                     JOIN person_references r ON r.subject_id=f.id
                     JOIN person_manual_instances i ON i.id=r.instance_id
                     JOIN person_review_decisions d ON d.instance_id=i.id AND d.subject_id=f.id
                     WHERE f.id=?1 AND f.folder_path=?2 AND f.revision=?3
                       AND f.identity_confirmed=1 AND d.decision='belongs'
                       AND i.needs_review=0 AND i.face_box IS NOT NULL
                       AND r.source_revision=i.source_revision",
                    params![
                        input.subject_id,
                        path_text(&input.folder_path),
                        input.expected_revision
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((name, asset, revision)) = source else {
                return Err(LibraryError::PersonConflict);
            };
            let previous: Option<String> = transaction
                .query_row(
                    "SELECT historical_person_id FROM folder_historical_links WHERE subject_id=?1",
                    [&input.subject_id],
                    |row| row.get(0),
                )
                .optional()?;
            if previous.is_some() && input.historical_person_id.is_none() {
                return Err(LibraryError::PersonConflict);
            }
            let id = if let Some(id) = &input.historical_person_id {
                let exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM historical_people WHERE id=?1)",
                    [id],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Err(LibraryError::MissingPersonRecord);
                }
                id.clone()
            } else {
                let id: String =
                    transaction
                        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))?;
                transaction.execute(
                    "INSERT INTO historical_people(id,display_name,reference_asset_path,reference_source_revision)
                     VALUES (?1,?2,?3,?4)",
                    params![id,name,asset,revision],
                )?;
                id
            };
            if previous.as_deref() == Some(id.as_str()) {
                return Err(LibraryError::PersonConflict);
            }
            transaction.execute(
                "INSERT INTO folder_historical_links(subject_id,historical_person_id) VALUES (?1,?2)
                 ON CONFLICT(subject_id) DO UPDATE SET historical_person_id=excluded.historical_person_id,
                 linked_at=unixepoch()",
                params![input.subject_id,id],
            )?;
            transaction.execute(
                "UPDATE folder_people SET revision=revision+1,updated_at=unixepoch() WHERE id=?1",
                [&input.subject_id],
            )?;
            transaction.execute(
                "INSERT INTO person_history_events(subject_id,historical_person_id,event_kind,request_id) VALUES (?1,?2,'link',?3)",
                params![input.subject_id,id,input.request_id],
            )?;
            transaction.execute(
                "INSERT INTO person_request_results VALUES (?1,'linkHistory',?2)",
                params![input.request_id, id],
            )?;
            crate::user::tags::reconcile_person_sources_for_subject(
                &transaction,
                &input.subject_id,
            )?;
            id
        };
        let result = transaction.query_row(
            "SELECT id,display_name,reference_asset_path,reference_source_revision,revision
             FROM historical_people WHERE id=?1",
            [history_id],
            row_historical_person,
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn unlink_historical_person(
        &self,
        input: &UnlinkHistoricalPerson,
    ) -> Result<(), LibraryError> {
        if input.request_id.is_empty() {
            return Err(LibraryError::InvalidPersonInstance);
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let replay: Option<(String, String)> = transaction
            .query_row(
                "SELECT operation,entity_id FROM person_request_results WHERE request_id=?1",
                [&input.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((operation, subject)) = replay {
            if operation != "unlinkHistory" || subject != input.subject_id {
                return Err(LibraryError::PersonConflict);
            }
        } else {
            let history_id: String = transaction
                .query_row(
                    "SELECT l.historical_person_id FROM folder_historical_links l
                 JOIN folder_people f ON f.id=l.subject_id
                 WHERE f.folder_path=?1 AND f.id=?2 AND f.revision=?3",
                    params![
                        path_text(&input.folder_path),
                        input.subject_id,
                        input.expected_revision
                    ],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or(LibraryError::PersonConflict)?;
            transaction.execute(
                "DELETE FROM folder_historical_links WHERE subject_id=?1",
                [&input.subject_id],
            )?;
            transaction.execute(
                "UPDATE folder_people SET revision=revision+1,updated_at=unixepoch() WHERE id=?1",
                [&input.subject_id],
            )?;
            transaction.execute(
                "INSERT INTO person_history_events(subject_id,historical_person_id,event_kind,request_id) VALUES (?1,?2,'unlink',?3)",
                params![input.subject_id,history_id,input.request_id],
            )?;
            transaction.execute(
                "INSERT INTO person_request_results VALUES (?1,'unlinkHistory',?2)",
                params![input.request_id, input.subject_id],
            )?;
            crate::user::tags::reconcile_person_sources_for_subject(
                &transaction,
                &input.subject_id,
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
    /// Intersect the entire directory snapshot before sorting/paging, never a loaded UI page.
    pub fn filter_assets_by_person(
        &self,
        folder: &Path,
        assets: &[AssetSummary],
        filter: &PersonFilter,
    ) -> Result<Vec<AssetSummary>, LibraryError> {
        let connection = self.read_connection();
        let mut statement = connection.prepare("SELECT i.asset_path,i.source_revision,i.needs_review,r.decision FROM person_manual_instances i LEFT JOIN person_review_decisions r ON r.instance_id=i.id AND r.subject_id=?2 WHERE i.folder_path=?1")?;
        let rows = statement
            .query_map(params![path_text(folder), filter.subject_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut by_path = std::collections::HashMap::<String, Vec<_>>::new();
        for (path, source, needs_review, decision) in rows {
            by_path
                .entry(path)
                .or_default()
                .push((source, needs_review, decision));
        }
        Ok(assets
            .iter()
            .filter(|asset| {
                let source = format!("{}:{}", asset.size_bytes, asset.modified_at_ms);
                let records = by_path.get(&path_text(&asset.path));
                if filter.state == PersonFilterState::Unassigned {
                    return records.is_none_or(|rows| {
                        !rows.iter().any(|(version, needs, decision)| {
                            version == &source && !needs && decision.is_some()
                        })
                    });
                }
                records.is_some_and(|rows| {
                    rows.iter().any(|(version, needs, decision)| {
                        let stale = version != &source || *needs;
                        if filter.state == PersonFilterState::NeedsReview {
                            return stale;
                        }
                        if stale {
                            return false;
                        }
                        match filter.state {
                            PersonFilterState::All => decision
                                .as_deref()
                                .is_some_and(|value| value != "doesNotBelong"),
                            PersonFilterState::Pending => decision.as_deref() == Some("pending"),
                            PersonFilterState::Belongs => decision.as_deref() == Some("belongs"),
                            PersonFilterState::DoesNotBelong => {
                                decision.as_deref() == Some("doesNotBelong")
                            }
                            PersonFilterState::Deferred => decision.as_deref() == Some("deferred"),
                            _ => false,
                        }
                    })
                })
            })
            .cloned()
            .collect())
    }

    pub fn update_person_instance(
        &self,
        input: &UpdatePersonInstance,
    ) -> Result<PersonInstance, LibraryError> {
        self.update_person_instance_with_source_identity(input, None)
    }

    pub fn update_person_instance_with_source_identity(
        &self,
        input: &UpdatePersonInstance,
        source_identity_revision: Option<&str>,
    ) -> Result<PersonInstance, LibraryError> {
        if input.request_id.is_empty()
            || input.source_revision.is_empty()
            || !valid_source_identity(source_identity_revision)
            || !valid_box(input.face_box)
            || !valid_box(input.body_box)
            || (input.face_box.is_none() && input.body_box.is_none())
        {
            return Err(LibraryError::InvalidPersonInstance);
        }
        let mut connection = self.connection.lock();
        let tx = connection.transaction()?;
        let replay: Option<(String, String)> = tx
            .query_row(
                "SELECT operation,entity_id FROM person_request_results WHERE request_id=?1",
                [&input.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((operation, entity)) = replay {
            if operation != "updateInstance" || entity != input.instance_id {
                return Err(LibraryError::PersonConflict);
            }
            let stored: Option<String> = tx.query_row(
                "SELECT source_identity_revision FROM person_manual_instances WHERE id=?1",
                [&input.instance_id],
                |row| row.get(0),
            )?;
            if stored.as_deref() != source_identity_revision {
                return Err(LibraryError::PersonConflict);
            }
        } else {
            let previous = tx.query_row("SELECT id,folder_path,asset_path,source_revision,face_box,body_box,needs_review,revision FROM person_manual_instances WHERE id=?1 AND folder_path=?2", params![input.instance_id,path_text(&input.folder_path)],row_instance).optional()?.ok_or(LibraryError::MissingPersonRecord)?;
            if previous.revision != input.expected_revision {
                return Err(LibraryError::PersonConflict);
            }
            tx.execute("INSERT INTO person_instance_events(instance_id,previous_json,request_id) VALUES (?1,?2,?3)",params![input.instance_id,serde_json::to_string(&previous)?,input.request_id])?;
            tx.execute("UPDATE person_manual_instances SET face_box=?1,body_box=?2,source_revision=?3,source_identity_revision=?4,needs_review=0,revision=revision+1,updated_at=unixepoch() WHERE id=?5",params![input.face_box.map(|v|serde_json::to_string(&v)).transpose()?,input.body_box.map(|v|serde_json::to_string(&v)).transpose()?,input.source_revision,source_identity_revision,input.instance_id])?;
            // A changed region is a new human claim: preserve old decisions in the audit,
            // and explicitly require review for every subject using this instance.
            tx.execute("INSERT INTO person_review_events(instance_id,subject_id,decision,revision,request_id) SELECT instance_id,subject_id,CASE WHEN decision='doesNotBelong' THEN decision ELSE 'pending' END,revision+1,?2 || ':' || subject_id FROM person_review_decisions WHERE instance_id=?1",params![input.instance_id,input.request_id])?;
            tx.execute("UPDATE person_review_decisions SET decision=CASE WHEN decision='doesNotBelong' THEN decision ELSE 'pending' END,revision=revision+1 WHERE instance_id=?1",[&input.instance_id])?;
            tx.execute("UPDATE folder_people SET revision=revision+1 WHERE id IN (SELECT subject_id FROM person_references WHERE instance_id=?1)",[&input.instance_id])?;
            tx.execute(
                "DELETE FROM person_references WHERE instance_id=?1",
                [&input.instance_id],
            )?;
            tx.execute(
                "INSERT INTO person_request_results VALUES (?1,'updateInstance',?2)",
                params![input.request_id, input.instance_id],
            )?;
            let subjects = {
                let mut statement = tx.prepare(
                    "SELECT subject_id FROM person_review_decisions WHERE instance_id=?1",
                )?;
                statement
                    .query_map([&input.instance_id], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            for subject in subjects {
                crate::user::tags::reconcile_person_source_for_subject_asset(
                    &tx,
                    &subject,
                    &path_text(&previous.asset_path),
                )?;
            }
        }
        let result = tx.query_row("SELECT id,folder_path,asset_path,source_revision,face_box,body_box,needs_review,revision FROM person_manual_instances WHERE id=?1 AND folder_path=?2",params![input.instance_id,path_text(&input.folder_path)],row_instance)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn reset_folder_person(&self, input: &ResetFolderPerson) -> Result<(), LibraryError> {
        if input.request_id.is_empty() {
            return Err(LibraryError::InvalidPersonInstance);
        }
        let mut connection = self.connection.lock();
        let tx = connection.transaction()?;
        let replay: Option<(String, String)> = tx
            .query_row(
                "SELECT operation,entity_id FROM person_request_results WHERE request_id=?1",
                [&input.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((operation, entity)) = replay {
            if operation != "resetPerson" || entity != input.subject_id {
                return Err(LibraryError::PersonConflict);
            }
        } else {
            if tx.execute("UPDATE folder_people SET display_name=?1,identity_confirmed=0,revision=revision+1,updated_at=unixepoch() WHERE id=?2 AND folder_path=?3 AND revision=?4",params![input.display_name.trim(),input.subject_id,path_text(&input.folder_path),input.expected_revision])? != 1 { return Err(LibraryError::PersonConflict); }
            tx.execute(
                "INSERT INTO person_history_events(subject_id,historical_person_id,event_kind,request_id)
                 SELECT subject_id,historical_person_id,'unlink',?2 FROM folder_historical_links WHERE subject_id=?1",
                params![input.subject_id,input.request_id],
            )?;
            tx.execute(
                "DELETE FROM folder_historical_links WHERE subject_id=?1",
                [&input.subject_id],
            )?;
            tx.execute(
                "DELETE FROM person_references WHERE subject_id=?1",
                [&input.subject_id],
            )?;
            tx.execute("INSERT INTO person_identity_events(subject_id,event_kind,display_name,revision,request_id) VALUES (?1,'reset',?2,?3,?4)",params![input.subject_id,input.display_name.trim(),input.expected_revision+1,input.request_id])?;
            tx.execute(
                "INSERT INTO person_request_results VALUES (?1,'resetPerson',?2)",
                params![input.request_id, input.subject_id],
            )?;
            crate::user::tags::reconcile_person_sources_for_subject(&tx, &input.subject_id)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_person_instance(
        &self,
        folder_path: &Path,
        id: &str,
    ) -> Result<PersonInstance, LibraryError> {
        let connection = self.read_connection();
        connection.query_row(
            "SELECT id,folder_path,asset_path,source_revision,face_box,body_box,needs_review,revision
             FROM person_manual_instances WHERE folder_path=?1 AND id=?2",
            params![path_text(folder_path),id],row_instance,
        ).optional()?.ok_or(LibraryError::MissingPersonRecord)
    }
    pub fn confirm_folder_person(
        &self,
        input: &ConfirmFolderPerson,
    ) -> Result<FolderPerson, LibraryError> {
        let name = input.display_name.trim();
        if name.is_empty() || input.request_id.is_empty() {
            return Err(LibraryError::InvalidPersonInstance);
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let replay: Option<(String, String)> = transaction
            .query_row(
                "SELECT operation,entity_id FROM person_request_results WHERE request_id=?1",
                [&input.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((operation, entity)) = replay {
            if operation != "confirmPerson" || entity != input.subject_id {
                return Err(LibraryError::PersonConflict);
            }
        } else {
            let reference: Option<String> = transaction
                .query_row(
                    "SELECT i.source_revision FROM person_manual_instances i
                 JOIN person_review_decisions r ON r.instance_id=i.id
                 WHERE i.id=?1 AND i.folder_path=?2 AND r.subject_id=?3
                   AND r.decision='belongs' AND i.face_box IS NOT NULL AND i.needs_review=0",
                    params![
                        input.reference_instance_id,
                        path_text(&input.folder_path),
                        input.subject_id
                    ],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(source_revision) = reference else {
                return Err(LibraryError::MissingPersonRecord);
            };
            let changed = transaction.execute(
                "UPDATE folder_people SET display_name=?1,identity_confirmed=1,revision=revision+1,
                 updated_at=unixepoch() WHERE id=?2 AND folder_path=?3 AND revision=?4",
                params![
                    name,
                    input.subject_id,
                    path_text(&input.folder_path),
                    input.expected_revision
                ],
            )?;
            if changed != 1 {
                return Err(LibraryError::PersonConflict);
            }
            transaction.execute(
                "DELETE FROM person_references WHERE subject_id=?1",
                [&input.subject_id],
            )?;
            transaction.execute(
                "INSERT OR REPLACE INTO person_references(subject_id,instance_id,source_revision)
                 VALUES (?1,?2,?3)",
                params![
                    input.subject_id,
                    input.reference_instance_id,
                    source_revision
                ],
            )?;
            transaction.execute(
                "INSERT INTO person_identity_events(subject_id,event_kind,display_name,revision,request_id)
                 VALUES (?1,'confirm',?2,?3,?4)",
                params![input.subject_id,name,input.expected_revision+1,input.request_id],
            )?;
            transaction.execute(
                "INSERT INTO person_request_results VALUES (?1,'confirmPerson',?2)",
                params![input.request_id, input.subject_id],
            )?;
        }
        let result = transaction
            .query_row(
                "SELECT id,folder_path,display_name,identity_confirmed,revision,
             (SELECT instance_id FROM person_references WHERE subject_id=folder_people.id LIMIT 1), (SELECT count(*) FROM person_review_decisions WHERE subject_id=folder_people.id AND decision='pending') FROM folder_people
             WHERE id=?1 AND folder_path=?2",
                params![input.subject_id, path_text(&input.folder_path)],
                row_person,
            )
            .optional()?
            .ok_or(LibraryError::MissingPersonRecord)?;
        transaction.commit()?;
        Ok(result)
    }
    pub fn list_folder_people(
        &self,
        folder_path: &Path,
    ) -> Result<Vec<FolderPerson>, LibraryError> {
        let connection = self.read_connection();
        let mut query = connection.prepare(
            "SELECT id,folder_path,display_name,identity_confirmed,revision,
             (SELECT instance_id FROM person_references WHERE subject_id=folder_people.id LIMIT 1), (SELECT count(*) FROM person_review_decisions WHERE subject_id=folder_people.id AND decision='pending')
             FROM folder_people WHERE folder_path=?1 ORDER BY created_at,id",
        )?;
        Ok(query
            .query_map([path_text(folder_path)], row_person)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn create_folder_person(
        &self,
        folder_path: &Path,
        request_id: &str,
    ) -> Result<FolderPerson, LibraryError> {
        if request_id.is_empty() {
            return Err(LibraryError::InvalidPersonInstance);
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let existing: Option<(String, String)> = transaction
            .query_row(
                "SELECT operation,entity_id FROM person_request_results WHERE request_id=?1",
                [request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let id = if let Some((operation, id)) = existing {
            if operation != "createPerson" {
                return Err(LibraryError::PersonConflict);
            }
            id
        } else {
            let id: String =
                transaction.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))?;
            transaction.execute(
                "INSERT INTO folder_people(id,folder_path) VALUES (?1,?2)",
                params![id, path_text(folder_path)],
            )?;
            transaction.execute(
                "INSERT INTO person_request_results VALUES (?1,'createPerson',?2)",
                params![request_id, id],
            )?;
            id
        };
        let result = transaction.query_row(
            "SELECT id,folder_path,display_name,identity_confirmed,revision,
             (SELECT instance_id FROM person_references WHERE subject_id=folder_people.id LIMIT 1), (SELECT count(*) FROM person_review_decisions WHERE subject_id=folder_people.id AND decision='pending') FROM folder_people WHERE id=?1 AND folder_path=?2",
            params![id, path_text(folder_path)], row_person,
        ).optional()?.ok_or(LibraryError::MissingPersonRecord)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn create_person_instance(
        &self,
        input: &CreatePersonInstance,
    ) -> Result<PersonInstance, LibraryError> {
        self.create_person_instance_with_source_identity(input, None)
    }

    pub fn create_person_instance_with_source_identity(
        &self,
        input: &CreatePersonInstance,
        source_identity_revision: Option<&str>,
    ) -> Result<PersonInstance, LibraryError> {
        if input.source_revision.is_empty()
            || input.request_id.is_empty()
            || !valid_source_identity(source_identity_revision)
            || (input.face_box.is_none() && input.body_box.is_none())
            || !valid_box(input.face_box)
            || !valid_box(input.body_box)
        {
            return Err(LibraryError::InvalidPersonInstance);
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let existing: Option<(String, String)> = transaction
            .query_row(
                "SELECT operation,entity_id FROM person_request_results WHERE request_id=?1",
                [&input.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let id = if let Some((operation, id)) = existing {
            if operation != "createInstance" {
                return Err(LibraryError::PersonConflict);
            }
            let stored: Option<String> = transaction.query_row(
                "SELECT source_identity_revision FROM person_manual_instances WHERE id=?1",
                [&id],
                |row| row.get(0),
            )?;
            if stored.as_deref() != source_identity_revision {
                return Err(LibraryError::PersonConflict);
            }
            id
        } else {
            let id: String =
                transaction.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))?;
            transaction.execute(
                "INSERT INTO person_manual_instances(id,folder_path,asset_path,source_revision,source_identity_revision,face_box,body_box)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![id, path_text(&input.folder_path), path_text(&input.asset_path), input.source_revision,source_identity_revision,
                    input.face_box.map(|value| serde_json::to_string(&value)).transpose()?,
                    input.body_box.map(|value| serde_json::to_string(&value)).transpose()?],
            )?;
            transaction.execute(
                "INSERT INTO person_request_results VALUES (?1,'createInstance',?2)",
                params![input.request_id, id],
            )?;
            id
        };
        let result = transaction.query_row(
            "SELECT id,folder_path,asset_path,source_revision,face_box,body_box,needs_review,revision
             FROM person_manual_instances WHERE id=?1 AND folder_path=?2",
            params![id, path_text(&input.folder_path)], row_instance,
        ).optional()?.ok_or(LibraryError::MissingPersonRecord)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn list_person_instances(
        &self,
        folder_path: &Path,
        asset_path: &Path,
    ) -> Result<Vec<PersonInstance>, LibraryError> {
        let connection = self.read_connection();
        let mut query = connection.prepare(
            "SELECT id,folder_path,asset_path,source_revision,face_box,body_box,needs_review,revision
             FROM person_manual_instances WHERE folder_path=?1 AND asset_path=?2 ORDER BY created_at,id",
        )?;
        Ok(query
            .query_map(
                params![path_text(folder_path), path_text(asset_path)],
                row_instance,
            )?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_manual_person_anchors(
        &self,
        folder_path: &Path,
        asset_path: &Path,
    ) -> Result<Vec<ManualPersonAnchor>, LibraryError> {
        let connection = self.read_connection();
        let mut query = connection.prepare(
            "SELECT id,source_identity_revision,face_box,body_box,needs_review,revision
             FROM person_manual_instances WHERE folder_path=?1 AND asset_path=?2 ORDER BY id",
        )?;
        let rows = query.query_map(
            params![path_text(folder_path), path_text(asset_path)],
            |row| {
                let face: Option<String> = row.get(2)?;
                let body: Option<String> = row.get(3)?;
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
                    instance_id: row.get(0)?,
                    source_identity_revision: row.get(1)?,
                    face_box: parse(face)?,
                    body_box: parse(body)?,
                    needs_review: row.get::<_, i64>(4)? != 0,
                    revision: row.get(5)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn set_person_review(&self, input: &SetPersonReview) -> Result<PersonReview, LibraryError> {
        if input.request_id.is_empty() {
            return Err(LibraryError::InvalidPersonInstance);
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let replay: Option<(String, String)> = transaction
            .query_row(
                "SELECT operation,entity_id FROM person_request_results WHERE request_id=?1",
                [&input.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let key = format!("{}:{}", input.instance_id, input.subject_id);
        if let Some((operation, entity)) = replay {
            if operation != "setReview" || entity != key {
                return Err(LibraryError::PersonConflict);
            }
        } else {
            let present: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM person_manual_instances i JOIN folder_people p
                   ON p.folder_path=i.folder_path WHERE i.id=?1 AND p.id=?2 AND i.folder_path=?3)",
                params![
                    input.instance_id,
                    input.subject_id,
                    path_text(&input.folder_path)
                ],
                |row| row.get(0),
            )?;
            if !present {
                return Err(LibraryError::MissingPersonRecord);
            }
            let current: Option<i64> = transaction.query_row(
                "SELECT revision FROM person_review_decisions WHERE instance_id=?1 AND subject_id=?2",
                params![input.instance_id, input.subject_id], |row| row.get(0),
            ).optional()?;
            if current.unwrap_or(0) != input.expected_revision {
                return Err(LibraryError::PersonConflict);
            }
            transaction.execute(
                "INSERT INTO person_review_decisions(instance_id,subject_id,decision,revision)
                 VALUES (?1,?2,?3,?4)
                 ON CONFLICT(instance_id,subject_id) DO UPDATE SET decision=excluded.decision,
                   revision=excluded.revision,updated_at=unixepoch()",
                params![
                    input.instance_id,
                    input.subject_id,
                    decision_text(input.decision),
                    input.expected_revision + 1
                ],
            )?;
            if input.decision != PersonReviewDecision::Belongs {
                let removed = transaction.execute(
                    "DELETE FROM person_references WHERE subject_id=?1 AND instance_id=?2",
                    params![input.subject_id, input.instance_id],
                )?;
                if removed > 0 {
                    transaction.execute(
                        "UPDATE folder_people SET revision=revision+1 WHERE id=?1",
                        [&input.subject_id],
                    )?;
                }
            }
            transaction.execute(
                "INSERT INTO person_review_events(instance_id,subject_id,decision,revision,request_id)
                 VALUES (?1,?2,?3,?4,?5)",
                params![input.instance_id, input.subject_id, decision_text(input.decision), input.expected_revision+1, input.request_id],
            )?;
            transaction.execute(
                "INSERT INTO person_request_results VALUES (?1,'setReview',?2)",
                params![input.request_id, key],
            )?;
            let asset_path: String = transaction.query_row(
                "SELECT asset_path FROM person_manual_instances WHERE id=?1",
                [&input.instance_id],
                |row| row.get(0),
            )?;
            crate::user::tags::reconcile_person_source_for_subject_asset(
                &transaction,
                &input.subject_id,
                &asset_path,
            )?;
        }
        let (instance, decision, revision) = transaction.query_row(
            "SELECT i.id,i.folder_path,i.asset_path,i.source_revision,i.face_box,i.body_box,i.needs_review,i.revision,
                    r.decision,r.revision FROM person_review_decisions r
             JOIN person_manual_instances i ON i.id=r.instance_id
             WHERE r.instance_id=?1 AND r.subject_id=?2",
            params![input.instance_id,input.subject_id],
            |row| Ok((row_instance(row)?, row.get::<_, String>(8)?, row.get::<_, i64>(9)?)),
        )?;
        transaction.commit()?;
        Ok(PersonReview {
            instance,
            subject_id: input.subject_id.clone(),
            decision: parse_decision(&decision),
            revision,
        })
    }

    pub fn list_person_reviews(
        &self,
        folder_path: &Path,
        subject_id: &str,
    ) -> Result<Vec<PersonReview>, LibraryError> {
        let connection = self.read_connection();
        let mut query = connection.prepare(
            "SELECT i.id,i.folder_path,i.asset_path,i.source_revision,i.face_box,i.body_box,i.needs_review,i.revision,
                    COALESCE(r.decision,'pending'),COALESCE(r.revision,0)
             FROM person_manual_instances i
             JOIN person_review_decisions r ON r.instance_id=i.id AND r.subject_id=?2
             WHERE i.folder_path=?1 ORDER BY i.asset_path,i.id",
        )?;
        Ok(query
            .query_map(params![path_text(folder_path), subject_id], |row| {
                Ok(PersonReview {
                    instance: row_instance(row)?,
                    subject_id: subject_id.to_owned(),
                    decision: parse_decision(&row.get::<_, String>(8)?),
                    revision: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_reviews_are_scoped_idempotent_and_survive_reopen() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let folder = temporary.path().join("photos");
        let other_folder = temporary.path().join("other");
        let image = folder.join("group.jpg");
        let library = Library::open(&database).unwrap();
        let first = library.create_folder_person(&folder, "create-one").unwrap();
        let second = library.create_folder_person(&folder, "create-two").unwrap();
        assert_eq!(
            first,
            library.create_folder_person(&folder, "create-one").unwrap()
        );
        assert!(
            library
                .list_folder_people(&other_folder)
                .unwrap()
                .is_empty()
        );

        let instance = library
            .create_person_instance(&CreatePersonInstance {
                folder_path: folder.clone(),
                asset_path: image.clone(),
                source_revision: "10:20".into(),
                face_box: Some([0.1, 0.2, 0.2, 0.3]),
                body_box: None,
                request_id: "face-one".into(),
            })
            .unwrap();
        let other_instance = library
            .create_person_instance(&CreatePersonInstance {
                folder_path: folder.clone(),
                asset_path: image,
                source_revision: "10:20".into(),
                face_box: Some([0.6, 0.2, 0.2, 0.3]),
                body_box: None,
                request_id: "face-two".into(),
            })
            .unwrap();
        assert_ne!(instance.id, other_instance.id);
        assert!(
            library
                .list_person_reviews(&folder, &first.id)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            library.confirm_folder_person(&ConfirmFolderPerson {
                folder_path: folder.clone(),
                subject_id: first.id.clone(),
                reference_instance_id: instance.id.clone(),
                display_name: "Alex".into(),
                expected_revision: first.revision,
                request_id: "too-early".into(),
            }),
            Err(LibraryError::MissingPersonRecord)
        ));
        let request = SetPersonReview {
            folder_path: folder.clone(),
            instance_id: instance.id,
            subject_id: first.id.clone(),
            decision: PersonReviewDecision::Belongs,
            expected_revision: 0,
            request_id: "review-one".into(),
        };
        let review = library.set_person_review(&request).unwrap();
        assert_eq!(review.revision, 1);
        assert_eq!(review, library.set_person_review(&request).unwrap());
        assert!(matches!(
            library.set_person_review(&SetPersonReview {
                request_id: "stale".into(),
                decision: PersonReviewDecision::DoesNotBelong,
                ..request
            }),
            Err(LibraryError::PersonConflict)
        ));
        assert!(
            library
                .list_person_reviews(&folder, &second.id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            library
                .list_person_reviews(&folder, &first.id)
                .unwrap()
                .len(),
            1
        );

        let confirmed = library
            .confirm_folder_person(&ConfirmFolderPerson {
                folder_path: folder.clone(),
                subject_id: first.id.clone(),
                reference_instance_id: review.instance.id.clone(),
                display_name: "Alex".into(),
                expected_revision: first.revision,
                request_id: "confirm-one".into(),
            })
            .unwrap();
        assert!(confirmed.identity_confirmed);
        let link_request = LinkHistoricalPerson {
            folder_path: folder.clone(),
            subject_id: first.id.clone(),
            historical_person_id: None,
            expected_revision: confirmed.revision,
            request_id: "history-one".into(),
        };
        let history = library.link_historical_person(&link_request).unwrap();
        assert_eq!(history.display_name, "Alex");
        assert_eq!(history.reference_source_revision, "10:20");
        assert_eq!(
            history,
            library.link_historical_person(&link_request).unwrap()
        );
        assert_eq!(
            library.get_historical_link(&folder, &first.id).unwrap(),
            Some(history.id.clone())
        );
        assert!(matches!(
            library.link_historical_person(&LinkHistoricalPerson {
                request_id: "history-stale".into(),
                ..link_request
            }),
            Err(LibraryError::PersonConflict)
        ));
        drop(library);

        let reopened = Library::open(&database).unwrap();
        assert_eq!(
            reopened.list_person_reviews(&folder, &first.id).unwrap(),
            vec![review]
        );
        assert_eq!(
            reopened
                .list_folder_people(&folder)
                .unwrap()
                .into_iter()
                .find(|person| person.id == first.id),
            Some(FolderPerson {
                revision: confirmed.revision + 1,
                ..confirmed.clone()
            }),
        );
        assert_eq!(reopened.list_historical_people().unwrap(), vec![history]);
        assert_eq!(
            reopened
                .get_historical_link(&other_folder, &first.id)
                .unwrap(),
            None
        );
        let unlink = UnlinkHistoricalPerson {
            folder_path: folder.clone(),
            subject_id: first.id.clone(),
            expected_revision: confirmed.revision + 1,
            request_id: "unlink-history".into(),
        };
        reopened.unlink_historical_person(&unlink).unwrap();
        reopened.unlink_historical_person(&unlink).unwrap();
        assert_eq!(
            reopened.get_historical_link(&folder, &first.id).unwrap(),
            None
        );
        reopened
            .link_historical_person(&LinkHistoricalPerson {
                folder_path: folder.clone(),
                subject_id: first.id.clone(),
                historical_person_id: Some(
                    reopened.list_historical_people().unwrap()[0].id.clone(),
                ),
                expected_revision: confirmed.revision + 2,
                request_id: "relink-history".into(),
            })
            .unwrap();
        reopened
            .reset_folder_person(&ResetFolderPerson {
                folder_path: folder.clone(),
                subject_id: first.id.clone(),
                display_name: "Alex".into(),
                expected_revision: confirmed.revision + 3,
                request_id: "reset-history".into(),
            })
            .unwrap();
        assert_eq!(
            reopened.get_historical_link(&folder, &first.id).unwrap(),
            None
        );
    }

    #[test]
    fn filters_whole_snapshot_and_preserves_manual_history_after_corrections() {
        let library = Library::in_memory().unwrap();
        let folder = PathBuf::from("/photos");
        let subject = library.create_folder_person(&folder, "person").unwrap();
        let assets = (0..301)
            .map(|index| AssetSummary {
                id: index.to_string(),
                path: folder.join(format!("{index:03}.jpg")),
                name: format!("{index:03}.jpg"),
                extension: "jpg".into(),
                kind: oxy_domain::AssetKind::Jpeg,
                size_bytes: 10,
                modified_at_ms: 20,
                has_sidecar: false,
                rating: Some(5),
                color_label: None,
                pick_label: None,
            })
            .collect::<Vec<_>>();
        let mut instances = Vec::new();
        for (index, asset_index) in [0, 0, 300].iter().enumerate() {
            let instance = library
                .create_person_instance(&CreatePersonInstance {
                    folder_path: folder.clone(),
                    asset_path: assets[*asset_index].path.clone(),
                    source_revision: "10:20".into(),
                    face_box: Some([0.1, 0.1, 0.2, 0.2]),
                    body_box: None,
                    request_id: format!("instance-{index}"),
                })
                .unwrap();
            library
                .set_person_review(&SetPersonReview {
                    folder_path: folder.clone(),
                    instance_id: instance.id.clone(),
                    subject_id: subject.id.clone(),
                    decision: PersonReviewDecision::Pending,
                    expected_revision: 0,
                    request_id: format!("review-{index}"),
                })
                .unwrap();
            instances.push(instance);
        }
        let filter = PersonFilter {
            subject_id: Some(subject.id.clone()),
            state: PersonFilterState::Pending,
        };
        let filtered = library
            .filter_assets_by_person(&folder, &assets, &filter)
            .unwrap();
        assert_eq!(filtered.len(), 2); // Two instances in one photo must not duplicate the photo.
        let page = oxy_fs::page_assets(
            &filtered,
            &oxy_domain::AssetQuery {
                page_size: Some(1),
                ..Default::default()
            },
            1,
        );
        assert_eq!(page.total, 2);
        assert_eq!(page.items[0].id, "300"); // Candidate beyond the ordinary first 250.
        let decide = |index: usize, decision, revision, request: &str| {
            library
                .set_person_review(&SetPersonReview {
                    folder_path: folder.clone(),
                    instance_id: instances[index].id.clone(),
                    subject_id: subject.id.clone(),
                    decision,
                    expected_revision: revision,
                    request_id: request.into(),
                })
                .unwrap()
        };
        decide(0, PersonReviewDecision::Belongs, 1, "belongs");
        assert_eq!(
            library
                .filter_assets_by_person(&folder, &assets, &filter)
                .unwrap()
                .len(),
            2
        ); // Other face still pending.
        let confirmed = library
            .confirm_folder_person(&ConfirmFolderPerson {
                folder_path: folder.clone(),
                subject_id: subject.id.clone(),
                reference_instance_id: instances[0].id.clone(),
                display_name: "Alex".into(),
                expected_revision: 1,
                request_id: "confirm".into(),
            })
            .unwrap();
        assert_eq!(
            confirmed.reference_instance_id,
            Some(instances[0].id.clone())
        );
        decide(
            0,
            PersonReviewDecision::DoesNotBelong,
            2,
            "reject-reference",
        );
        let person = library.list_folder_people(&folder).unwrap().remove(0);
        assert!(person.reference_instance_id.is_none());
        assert!(person.identity_confirmed); // Identity is separate from its reference.
        decide(1, PersonReviewDecision::Deferred, 1, "defer");
        assert_eq!(
            library
                .filter_assets_by_person(&folder, &assets, &filter)
                .unwrap()
                .len(),
            1
        );
        let correction = UpdatePersonInstance {
            folder_path: folder.clone(),
            instance_id: instances[0].id.clone(),
            source_revision: "10:20".into(),
            face_box: Some([0.2, 0.2, 0.3, 0.3]),
            body_box: Some([0.1, 0.1, 0.5, 0.8]),
            expected_revision: 1,
            request_id: "correct".into(),
        };
        let updated = library.update_person_instance(&correction).unwrap();
        assert_eq!(
            updated,
            library.update_person_instance(&correction).unwrap()
        );
        assert!(matches!(
            library.update_person_instance(&UpdatePersonInstance {
                request_id: "stale-correct".into(),
                ..correction
            }),
            Err(LibraryError::PersonConflict)
        ));
        assert_eq!(
            library
                .list_person_reviews(&folder, &subject.id)
                .unwrap()
                .iter()
                .find(|r| r.instance.id == instances[0].id)
                .unwrap()
                .decision,
            PersonReviewDecision::DoesNotBelong
        ); // Never silently revive explicit negatives.
        let mut changed = assets;
        changed[300].modified_at_ms = 21;
        assert!(
            library
                .filter_assets_by_person(&folder, &changed, &filter)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            library
                .filter_assets_by_person(
                    &folder,
                    &changed,
                    &PersonFilter {
                        state: PersonFilterState::NeedsReview,
                        ..filter
                    }
                )
                .unwrap()
                .len(),
            1
        );
        library
            .reset_folder_person(&ResetFolderPerson {
                folder_path: folder.clone(),
                subject_id: subject.id.clone(),
                display_name: "Temporary".into(),
                expected_revision: person.revision,
                request_id: "reset".into(),
            })
            .unwrap();
        assert!(!library.list_folder_people(&folder).unwrap()[0].identity_confirmed);
        assert_eq!(
            library
                .list_person_reviews(&folder, &subject.id)
                .unwrap()
                .len(),
            3
        );
        let connection = library.read_connection();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM person_instance_events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn rejects_invalid_boxes_and_cross_folder_subjects() {
        let library = Library::in_memory().unwrap();
        let folder = PathBuf::from("/one");
        let subject = library.create_folder_person(&folder, "subject").unwrap();
        let invalid = CreatePersonInstance {
            folder_path: folder.clone(),
            asset_path: folder.join("a.jpg"),
            source_revision: "1:2".into(),
            face_box: Some([0.9, 0.0, 0.2, 0.3]),
            body_box: None,
            request_id: "bad".into(),
        };
        assert!(matches!(
            library.create_person_instance(&invalid),
            Err(LibraryError::InvalidPersonInstance)
        ));
        let instance = library
            .create_person_instance(&CreatePersonInstance {
                face_box: Some([0.0, 0.0, 0.2, 0.3]),
                request_id: "good".into(),
                ..invalid
            })
            .unwrap();
        assert!(matches!(
            library.set_person_review(&SetPersonReview {
                folder_path: PathBuf::from("/other"),
                instance_id: instance.id,
                subject_id: subject.id,
                decision: PersonReviewDecision::Belongs,
                expected_revision: 0,
                request_id: "cross-folder".into(),
            }),
            Err(LibraryError::MissingPersonRecord)
        ));
    }

    #[test]
    fn full_source_identity_is_persisted_only_when_observed() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let library = Library::open(&database).unwrap();
        let folder = PathBuf::from("/photos");
        let legacy = CreatePersonInstance {
            folder_path: folder.clone(),
            asset_path: folder.join("legacy.jpg"),
            source_revision: "10:20".into(),
            face_box: Some([0.1, 0.1, 0.2, 0.2]),
            body_box: None,
            request_id: "legacy".into(),
        };
        let old = library.create_person_instance(&legacy).unwrap();
        assert_eq!(
            library
                .list_manual_person_anchors(&folder, &legacy.asset_path)
                .unwrap()[0]
                .source_identity_revision,
            None
        );
        let revision = "a".repeat(64);
        let fresh = CreatePersonInstance {
            asset_path: folder.join("fresh.jpg"),
            request_id: "fresh".into(),
            ..legacy.clone()
        };
        let created = library
            .create_person_instance_with_source_identity(&fresh, Some(&revision))
            .unwrap();
        assert_eq!(
            library
                .create_person_instance_with_source_identity(&fresh, Some(&revision))
                .unwrap(),
            created
        );
        assert!(matches!(
            library.create_person_instance_with_source_identity(&fresh, Some(&"b".repeat(64))),
            Err(LibraryError::PersonConflict)
        ));
        let update = UpdatePersonInstance {
            folder_path: folder.clone(),
            instance_id: old.id,
            source_revision: "10:20".into(),
            face_box: Some([0.2, 0.2, 0.2, 0.2]),
            body_box: None,
            expected_revision: 1,
            request_id: "rebind".into(),
        };
        library
            .update_person_instance_with_source_identity(&update, Some(&revision))
            .unwrap();
        drop(library);
        let reopened = Library::open(&database).unwrap();
        for path in [&legacy.asset_path, &fresh.asset_path] {
            let anchors = reopened.list_manual_person_anchors(&folder, path).unwrap();
            assert_eq!(
                anchors[0].source_identity_revision.as_deref(),
                Some(revision.as_str())
            );
        }
    }

    #[test]
    fn older_manual_table_migrates_without_assigning_unproven_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE person_manual_instances (
               id TEXT PRIMARY KEY,folder_path TEXT NOT NULL,asset_path TEXT NOT NULL,
               source_revision TEXT NOT NULL,face_box TEXT,body_box TEXT,
               needs_review INTEGER NOT NULL DEFAULT 0,revision INTEGER NOT NULL DEFAULT 1,
               created_at INTEGER NOT NULL DEFAULT (unixepoch()),
               updated_at INTEGER NOT NULL DEFAULT (unixepoch()));
             INSERT INTO person_manual_instances(id,folder_path,asset_path,source_revision,face_box)
               VALUES ('old','/photos','/photos/a.jpg','10:20','[0.1,0.1,0.2,0.2]');",
            )
            .unwrap();
        drop(connection);
        let library = Library::open(&database).unwrap();
        let anchors = library
            .list_manual_person_anchors(Path::new("/photos"), Path::new("/photos/a.jpg"))
            .unwrap();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].instance_id, "old");
        assert_eq!(anchors[0].source_identity_revision, None);
    }
}
