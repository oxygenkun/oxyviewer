//! Statements only. Identity migration, review and matching policy live in oxy-people.
use crate::StoreError;
use oxy_domain::{GlobalPerson, GlobalPersonFolder, PersonReviewDecision, PersonTuple};
use rusqlite::{Connection, OptionalExtension, params};

pub fn catalog(c: &Connection) -> Result<Vec<GlobalPerson>, StoreError> {
    let mut s =
        c.prepare("SELECT id,display_name,revision,(SELECT tag_id FROM global_person_tags WHERE person_id=global_people.id) AS tag_id FROM global_people ORDER BY display_name,id")?;
    let mut people = s
        .query_map([], |r| {
            Ok(GlobalPerson {
                id: r.get("id")?,
                display_name: r.get("display_name")?,
                revision: r.get("revision")?,
                reference_instance_ids: Vec::new(),
                tag_id: r.get("tag_id")?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for p in &mut people {
        let mut s = c.prepare("SELECT instance_id FROM global_person_references WHERE person_id=?1 ORDER BY instance_id")?;
        p.reference_instance_ids = s
            .query_map([&p.id], |r| r.get("instance_id"))?
            .collect::<Result<_, _>>()?;
    }
    Ok(people)
}

pub fn insert_person(c: &Connection, id: &str, name: &str) -> Result<(), StoreError> {
    c.execute(
        "INSERT OR IGNORE INTO global_people(id,display_name) VALUES (?1,?2)",
        params![id, name],
    )?;
    Ok(())
}
pub fn rename(c: &Connection, id: &str, name: &str, revision: i64) -> Result<usize, StoreError> {
    Ok(c.execute(
        "UPDATE global_people SET display_name=?2,revision=revision+1 WHERE id=?1 AND revision=?3",
        params![id, name, revision],
    )?)
}
pub fn bump(c: &Connection, id: &str) -> Result<(), StoreError> {
    c.execute(
        "UPDATE global_people SET revision=revision+1 WHERE id=?1",
        [id],
    )?;
    Ok(())
}

pub fn set_tag(c: &Connection, person: &str, tag: Option<i64>) -> Result<(), StoreError> {
    c.execute("INSERT INTO global_person_tags(person_id,tag_id) VALUES(?1,?2) ON CONFLICT(person_id) DO UPDATE SET tag_id=excluded.tag_id",params![person,tag])?;
    Ok(())
}
pub fn migrate_tag(c: &Connection, person: &str) -> Result<(), StoreError> {
    c.execute("INSERT OR IGNORE INTO global_person_tags(person_id,tag_id) SELECT historical_person_id,CASE WHEN enabled=1 THEN tag_id ELSE NULL END FROM person_tag_links WHERE historical_person_id=?1",[person])?;
    Ok(())
}

pub struct LegacySubject {
    pub folder: String,
    pub id: String,
    pub history_id: Option<String>,
    pub name: String,
}
pub fn migration_needed(c: &Connection) -> Result<bool, StoreError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM folder_people p WHERE NOT EXISTS(SELECT 1 FROM global_person_migrations m WHERE m.subject_id=p.id)) OR EXISTS(SELECT 1 FROM historical_people h WHERE NOT EXISTS(SELECT 1 FROM global_people g WHERE g.id=h.id)) AS needed",[],|r|r.get("needed"))?)
}
pub fn legacy_subjects(c: &Connection) -> Result<Vec<LegacySubject>, StoreError> {
    let mut s=c.prepare("SELECT p.id,p.folder_path,l.historical_person_id,COALESCE(h.display_name,p.display_name,'未命名人物') AS name FROM folder_people p LEFT JOIN folder_historical_links l ON l.subject_id=p.id LEFT JOIN historical_people h ON h.id=l.historical_person_id WHERE NOT EXISTS(SELECT 1 FROM global_person_migrations m WHERE m.subject_id=p.id) ORDER BY p.id")?;
    Ok(s.query_map([], |r| {
        Ok(LegacySubject {
            folder: r.get("folder_path")?,
            id: r.get("id")?,
            history_id: r.get("historical_person_id")?,
            name: r.get("name")?,
        })
    })?
    .collect::<Result<_, _>>()?)
}
pub fn legacy_references(c: &Connection, subject: &str) -> Result<Vec<String>, StoreError> {
    let mut s = c.prepare("SELECT instance_id FROM person_references WHERE subject_id=?1")?;
    Ok(s.query_map([subject], |r| r.get("instance_id"))?
        .collect::<Result<_, _>>()?)
}
pub fn mark_migrated(c: &Connection, subject: &str, person: &str) -> Result<(), StoreError> {
    c.execute(
        "INSERT INTO global_person_migrations(subject_id,person_id) VALUES(?1,?2)",
        params![subject, person],
    )?;
    Ok(())
}
pub fn event(c: &Connection, id: &str) -> Result<Option<(String, String)>, StoreError> {
    Ok(c.query_row(
        "SELECT payload,result FROM global_person_events WHERE request_id=?1",
        [id],
        |r| Ok((r.get("payload")?, r.get("result")?)),
    )
    .optional()?)
}
pub fn record_event(
    c: &Connection,
    id: &str,
    payload: &str,
    result: &str,
) -> Result<(), StoreError> {
    c.execute(
        "INSERT INTO global_person_events(request_id,payload,result) VALUES(?1,?2,?3)",
        params![id, payload, result],
    )?;
    Ok(())
}
pub fn tuples(c: &Connection, folder: Option<&str>) -> Result<Vec<PersonTuple>, StoreError> {
    let mut s=c.prepare("SELECT i.*,t.person_id,r.decision FROM person_manual_instances i LEFT JOIN global_person_targets t ON t.instance_id=i.id LEFT JOIN global_person_reviews r ON r.instance_id=i.id AND r.person_id=t.person_id WHERE (?1 IS NULL OR i.folder_path=?1) ORDER BY i.asset_path,i.id")?;
    Ok(s.query_map([folder], |r| {
        let face: Option<String> = r.get("face_box")?;
        let body: Option<String> = r.get("body_box")?;
        let decision: Option<String> = r.get("decision")?;
        Ok(PersonTuple {
            id: r.get("id")?,
            asset_path: r.get::<_, String>("asset_path")?.into(),
            source_revision: r.get("source_revision")?,
            source_identity_revision: r
                .get::<_, Option<String>>("source_identity_revision")?
                .unwrap_or_default(),
            face_box: face.and_then(|x| serde_json::from_str(&x).ok()),
            body_box: body.and_then(|x| serde_json::from_str(&x).ok()),
            revision: r.get("revision")?,
            needs_review: r.get("needs_review")?,
            person_id: r.get("person_id")?,
            decision: decision.map(|d| PersonReviewDecision::from_text(&d)),
            score: None,
        })
    })?
    .collect::<Result<_, _>>()?)
}

pub fn confirmed_gallery_counts(c: &Connection, person: &str) -> Result<(u64, u64), StoreError> {
    Ok(c.query_row(
        "SELECT COUNT(*) AS total, COUNT(DISTINCT i.folder_path) AS folders FROM person_manual_instances i JOIN global_person_reviews r ON r.instance_id=i.id WHERE r.person_id=?1 AND r.decision='belongs'",
        [person],
        |r| Ok((r.get("total")?, r.get("folders")?)),
    )?)
}

pub fn confirmed_gallery(
    c: &Connection,
    person: &str,
    offset: u32,
    limit: u32,
) -> Result<Vec<PersonTuple>, StoreError> {
    let mut s = c.prepare("SELECT i.* FROM person_manual_instances i JOIN global_person_reviews r ON r.instance_id=i.id WHERE r.person_id=?1 AND r.decision='belongs' ORDER BY i.asset_path,i.id LIMIT ?2 OFFSET ?3")?;
    Ok(s.query_map(params![person, limit, offset], |r| {
        let face: Option<String> = r.get("face_box")?;
        let body: Option<String> = r.get("body_box")?;
        Ok(PersonTuple {
            id: r.get("id")?,
            asset_path: r.get::<_, String>("asset_path")?.into(),
            source_revision: r.get("source_revision")?,
            source_identity_revision: r
                .get::<_, Option<String>>("source_identity_revision")?
                .unwrap_or_default(),
            face_box: face.and_then(|x| serde_json::from_str(&x).ok()),
            body_box: body.and_then(|x| serde_json::from_str(&x).ok()),
            revision: r.get("revision")?,
            needs_review: r.get("needs_review")?,
            person_id: Some(person.into()),
            decision: Some(PersonReviewDecision::Belongs),
            score: None,
        })
    })?
    .collect::<Result<_, _>>()?)
}

pub fn confirmed_folders(
    c: &Connection,
    person: &str,
    offset: u32,
    limit: u32,
) -> Result<Vec<GlobalPersonFolder>, StoreError> {
    let mut statement = c.prepare(
        "SELECT i.folder_path,MIN(i.asset_path) AS cover_asset_path,
         COUNT(DISTINCT i.asset_path) AS photo_count,COUNT(*) AS instance_count
         FROM person_manual_instances i JOIN global_person_reviews r ON r.instance_id=i.id
         WHERE r.person_id=?1 AND r.decision='belongs'
         GROUP BY i.folder_path ORDER BY i.folder_path LIMIT ?2 OFFSET ?3",
    )?;
    Ok(statement
        .query_map(params![person, limit, offset], |row| {
            Ok(GlobalPersonFolder {
                folder_path: row.get::<_, String>("folder_path")?.into(),
                cover_asset_path: row.get::<_, String>("cover_asset_path")?.into(),
                photo_count: row.get("photo_count")?,
                instance_count: row.get("instance_count")?,
            })
        })?
        .collect::<Result<_, _>>()?)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Review {
    pub instance_id: String,
    pub person_id: String,
    pub decision: PersonReviewDecision,
    pub revision: i64,
}
pub fn reviews(c: &Connection, folder: Option<&str>) -> Result<Vec<Review>, StoreError> {
    let mut s=c.prepare("SELECT r.* FROM global_person_reviews r JOIN person_manual_instances i ON i.id=r.instance_id WHERE (?1 IS NULL OR i.folder_path=?1) ORDER BY r.instance_id,r.person_id")?;
    Ok(s.query_map([folder], |r| {
        Ok(Review {
            instance_id: r.get("instance_id")?,
            person_id: r.get("person_id")?,
            decision: PersonReviewDecision::from_text(&r.get::<_, String>("decision")?),
            revision: r.get("revision")?,
        })
    })?
    .collect::<Result<_, _>>()?)
}
pub fn review(
    c: &Connection,
    instance: &str,
    person: &str,
    decision: PersonReviewDecision,
) -> Result<(), StoreError> {
    c.execute("INSERT INTO global_person_reviews(instance_id,person_id,decision) VALUES (?1,?2,?3) ON CONFLICT(instance_id,person_id) DO UPDATE SET decision=excluded.decision,revision=global_person_reviews.revision+1",params![instance,person,decision.as_str()])?;
    Ok(())
}
pub fn target(c: &Connection, instance: &str, person: &str) -> Result<(), StoreError> {
    c.execute("INSERT INTO global_person_targets(instance_id,person_id) VALUES(?1,?2) ON CONFLICT(instance_id) DO UPDATE SET person_id=excluded.person_id",params![instance,person])?;
    Ok(())
}
pub fn add_reference(c: &Connection, person: &str, instance: &str) -> Result<(), StoreError> {
    c.execute(
        "INSERT OR IGNORE INTO global_person_references(person_id,instance_id) VALUES(?1,?2)",
        params![person, instance],
    )?;
    Ok(())
}
pub fn remove_reference(c: &Connection, person: &str, instance: &str) -> Result<(), StoreError> {
    c.execute(
        "DELETE FROM global_person_references WHERE person_id=?1 AND instance_id=?2",
        params![person, instance],
    )?;
    Ok(())
}
pub fn suggestions(c: &Connection, folder: &str) -> Result<Option<String>, StoreError> {
    Ok(c.query_row("SELECT s.payload FROM global_person_suggestions s JOIN person_analysis_heads h ON h.folder_path=s.folder_path AND h.run_id=s.run_id JOIN person_analysis_runs r ON r.run_id=h.run_id AND r.state='completed' WHERE s.folder_path=?1",[folder],|r|r.get("payload")).optional()?)
}
pub fn save_suggestions(
    c: &Connection,
    folder: &str,
    run: &str,
    payload: &str,
) -> Result<(), StoreError> {
    c.execute("INSERT INTO global_person_suggestions(folder_path,run_id,payload) VALUES(?1,?2,?3) ON CONFLICT(folder_path) DO UPDATE SET run_id=excluded.run_id,payload=excluded.payload",params![folder,run,payload])?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceVector {
    pub person_id: String,
    pub tuple: PersonTuple,
    pub vector: Vec<u8>,
    pub face_box: [f64; 4],
}
/// Same-source candidate vectors; the domain validates unique numeric geometry correspondence.
pub fn reference_vectors(
    c: &Connection,
    space: &str,
    producer: &str,
) -> Result<Vec<ReferenceVector>, StoreError> {
    let all = tuples(c, None)?;
    let mut s=c.prepare("SELECT g.person_id,g.instance_id,f.vector,d.face_box FROM global_person_references g JOIN person_manual_instances i ON i.id=g.instance_id JOIN global_person_reviews r ON r.instance_id=i.id AND r.person_id=g.person_id AND r.decision='belongs' JOIN person_instances_cache d ON d.asset_path=i.asset_path AND d.source_revision=i.source_identity_revision AND d.face_box IS NOT NULL JOIN person_features_cache f ON f.asset_path=d.asset_path AND f.instance_id=d.instance_id AND f.source_revision=d.source_revision AND f.feature_space_id=?1 JOIN person_feature_spaces fs ON fs.id=f.feature_space_id AND fs.producer_fingerprint=?2 WHERE i.needs_review=0 ORDER BY g.person_id,g.instance_id")?;
    let rows = s
        .query_map(params![space, producer], |r| {
            Ok((
                r.get::<_, String>("person_id")?,
                r.get::<_, String>("instance_id")?,
                r.get::<_, Vec<u8>>("vector")?,
                r.get::<_, String>("face_box")?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut result = Vec::new();
    for (person_id, id, vector, face) in rows {
        if let Some(tuple) = all.iter().find(|t| t.id == id) {
            result.push(ReferenceVector {
                person_id,
                tuple: tuple.clone(),
                vector,
                face_box: serde_json::from_str(&face)?,
            });
        }
    }
    Ok(result)
}
