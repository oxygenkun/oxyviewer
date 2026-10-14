//! Person review policy; the application workflow owns the surrounding transaction.
use super::{failure, requests, workspace};
use crate::people::{PeopleError, repo::global as db, types::*};
use crate::storage::{Connection, Store};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub(crate) enum PreparedReview {
    Replayed(usize),
    Pending { payload: String, folder: PathBuf },
}

pub(crate) struct ReviewOutcome {
    pub changed: usize,
    pub affected_people: HashSet<String>,
}

pub(crate) fn prepare(
    store: &Store,
    input: &ReviewPersonTuples,
) -> Result<PreparedReview, PeopleError> {
    let payload = requests::payload("reviewTuples", input)?;
    if let Some(result) = requests::replay(&store.read(), &input.request_id, &payload)? {
        return Ok(PreparedReview::Replayed(result));
    }
    if input.tuples.is_empty() || input.tuples.len() > 5000 {
        return Err(failure("请选择 1–5000 个人物实例"));
    }
    let folder = input.folder_path.canonicalize()?;
    let mut seen = HashSet::new();
    for tuple in &input.tuples {
        if !seen.insert(&tuple.id)
            || tuple.needs_review
            || tuple.asset_path.parent() != Some(folder.as_path())
            || crate::files::observe_file(&tuple.asset_path)?.revision_id()
                != tuple.source_identity_revision
        {
            return Err(failure("实例或源图已变化，请刷新后重新核对"));
        }
    }
    Ok(PreparedReview::Pending { payload, folder })
}

pub(crate) fn apply(
    connection: &Connection,
    folder: &Path,
    input: &ReviewPersonTuples,
) -> Result<ReviewOutcome, PeopleError> {
    let current = workspace(connection, folder)?;
    if current.revision != input.workspace_revision {
        return Err(PeopleError::PersonConflict);
    }
    if !db::catalog(connection)?
        .iter()
        .any(|person| person.id == input.person_id)
    {
        return Err(PeopleError::MissingPersonRecord);
    }
    let manual = db::tuples(connection, Some(&folder.to_string_lossy()))?;
    let reviews = db::reviews(connection, Some(&folder.to_string_lossy()))?;
    let mut affected_people = HashSet::from([input.person_id.clone()]);
    let mut changed = 0;
    for tuple in &input.tuples {
        if !current
            .groups
            .iter()
            .flat_map(|group| &group.members)
            .any(|t| t == tuple)
        {
            return Err(PeopleError::PersonConflict);
        }
        // A transfer must not erase an explicit rejection or another decision.
        if input.decision == PersonReviewDecision::Pending
            && reviews
                .iter()
                .any(|review| review.instance_id == tuple.id && review.person_id == input.person_id)
        {
            continue;
        }
        let others: Vec<_> = reviews
            .iter()
            .filter(|review| {
                review.instance_id == tuple.id
                    && review.person_id != input.person_id
                    && review.decision == PersonReviewDecision::Belongs
            })
            .collect();
        if input.decision != PersonReviewDecision::DoesNotBelong
            && !others.is_empty()
            && !input.replace_confirmed
        {
            return Err(failure(
                "选中实例已有其他已确认人物；请明确选择“替换已有归属”后再指定",
            ));
        }
        if input.decision != PersonReviewDecision::DoesNotBelong && input.replace_confirmed {
            for other in others {
                affected_people.insert(other.person_id.clone());
                db::review(
                    connection,
                    &tuple.id,
                    &other.person_id,
                    PersonReviewDecision::DoesNotBelong,
                )?;
                db::remove_reference(connection, &other.person_id, &tuple.id)?;
                db::bump(connection, &other.person_id)?;
            }
        }
        // Bulk transfer to pending cannot undo an explicit previous decision.
        if !manual.iter().any(|t| t.id == tuple.id) {
            crate::people::repo::manual::insert_instance(
                connection,
                &crate::people::repo::manual::NewInstance {
                    id: &tuple.id,
                    folder_path: &folder.to_string_lossy(),
                    asset_path: &tuple.asset_path.to_string_lossy(),
                    source_revision: &tuple.source_revision,
                    source_identity_revision: Some(&tuple.source_identity_revision),
                    face_box_json: tuple
                        .face_box
                        .map(|b| serde_json::to_string(&b))
                        .transpose()?
                        .as_deref(),
                    body_box_json: tuple
                        .body_box
                        .map(|b| serde_json::to_string(&b))
                        .transpose()?
                        .as_deref(),
                },
            )?;
        }
        db::review(connection, &tuple.id, &input.person_id, input.decision)?;
        if input.decision != PersonReviewDecision::DoesNotBelong
            || tuple
                .person_id
                .as_ref()
                .is_none_or(|id| *id == input.person_id)
        {
            db::target(connection, &tuple.id, &input.person_id)?;
        }
        if input.decision != PersonReviewDecision::Belongs {
            db::remove_reference(connection, &input.person_id, &tuple.id)?;
        }
        changed += 1;
    }
    db::bump(connection, &input.person_id)?;
    Ok(ReviewOutcome {
        changed,
        affected_people,
    })
}
