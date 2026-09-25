//! The tag vocabulary, the assignments, and the XMP mirror.
//!
//! Two tables hold an assignment: `asset_tag_sources` records *why* an asset
//! carries a tag (the user said so, a sidecar said so, a person identity says
//! so), and `asset_tags` is the effective set the app reads. [`reconcile_effective`]
//! is the statement that keeps the second derived from the first; the rules
//! about which sources a given user action may add or drop are not here.

use crate::StoreError;
use oxy_domain::{CustomTag, CustomTagId};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};

/// One asset row in the pending XMP mirror write queue.
pub fn enqueue_sync(connection: &Connection, paths: &[String]) -> Result<(), StoreError> {
    for path in paths {
        connection.execute(
            "INSERT INTO tag_xmp_sync_queue(asset_path, requested_at, attempt_count, last_error)
             VALUES (?1, unixepoch(), 0, NULL)
             ON CONFLICT(asset_path) DO UPDATE SET
               requested_at=excluded.requested_at, attempt_count=0, last_error=NULL",
            params![path],
        )?;
    }
    Ok(())
}

/// Whether the effective set currently contains this pair.
pub fn effective_assignment_present(
    connection: &Connection,
    asset_path: &str,
    tag_id: CustomTagId,
) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM asset_tags WHERE asset_path=?1 AND tag_id=?2) AS present",
        params![asset_path, tag_id],
        |row| row.get("present"),
    )?)
}

/// Adds the pair to the effective set, keeping the original assignment time.
pub fn insert_effective_assignment(
    connection: &Connection,
    asset_path: &str,
    tag_id: CustomTagId,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT OR IGNORE INTO asset_tags(asset_path,tag_id) VALUES (?1,?2)",
        params![asset_path, tag_id],
    )?;
    Ok(())
}

/// Drops the pair from the effective set.
pub fn delete_effective_assignment(
    connection: &Connection,
    asset_path: &str,
    tag_id: CustomTagId,
) -> Result<(), StoreError> {
    connection.execute(
        "DELETE FROM asset_tags WHERE asset_path=?1 AND tag_id=?2",
        params![asset_path, tag_id],
    )?;
    Ok(())
}

/// Makes the effective set agree with the recorded sources for one pair.
///
/// This is the only statement that writes `asset_tags` from
/// `asset_tag_sources`, so a source can be added or dropped anywhere without
/// the derived set drifting.
pub fn reconcile_effective(
    connection: &Connection,
    asset_path: &str,
    tag_id: CustomTagId,
) -> Result<(), StoreError> {
    if effective_source_present(connection, asset_path, tag_id)? {
        insert_effective_assignment(connection, asset_path, tag_id)
    } else {
        delete_effective_assignment(connection, asset_path, tag_id)
    }
}

/// Whether any source at all records this pair.
pub fn effective_source_present(
    connection: &Connection,
    asset_path: &str,
    tag_id: CustomTagId,
) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM asset_tag_sources WHERE asset_path=?1 AND tag_id=?2) AS present",
        params![asset_path, tag_id],
        |row| row.get("present"),
    )?)
}

/// Whether the vocabulary still contains this tag.
pub fn tag_exists(connection: &Connection, tag_id: CustomTagId) -> Result<bool, StoreError> {
    Ok(connection
        .query_row("SELECT 1 FROM custom_tags WHERE id = ?1", params![tag_id], |_| {
            Ok(true)
        })
        .optional()?
        .unwrap_or(false))
}

/// Every tag id recorded as a source of this pair, for the given kind.
pub fn source_tag_ids_of(
    connection: &Connection,
    asset_path: &str,
    source_kind: &str,
    source_id: &str,
) -> Result<HashSet<CustomTagId>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT tag_id FROM asset_tag_sources
         WHERE asset_path=?1 AND source_kind=?2 AND source_id=?3",
    )?;
    Ok(statement
        .query_map(params![asset_path, source_kind, source_id], |row| {
            row.get::<_, CustomTagId>("tag_id")
        })?
        .collect::<Result<HashSet<_>, _>>()?)
}

/// Every tag id recorded as a source of this asset, for the given kind.
pub fn source_tag_ids_for_kind(
    connection: &Connection,
    asset_path: &str,
    source_kind: &str,
) -> Result<Vec<CustomTagId>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT tag_id FROM asset_tag_sources
         WHERE asset_path=?1 AND source_kind=?2",
    )?;
    Ok(statement
        .query_map(params![asset_path, source_kind], |row| {
            row.get::<_, CustomTagId>("tag_id")
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Records one reason for an assignment. Ignored when already recorded.
pub fn insert_source(
    connection: &Connection,
    asset_path: &str,
    tag_id: CustomTagId,
    source_kind: &str,
    source_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT OR IGNORE INTO asset_tag_sources(asset_path,tag_id,source_kind,source_id)
         VALUES (?1,?2,?3,?4)",
        params![asset_path, tag_id, source_kind, source_id],
    )?;
    Ok(())
}

/// Drops every reason of one kind for a pair, whatever its source id.
pub fn delete_source_of_kind(
    connection: &Connection,
    asset_path: &str,
    tag_id: CustomTagId,
    source_kind: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "DELETE FROM asset_tag_sources WHERE asset_path=?1 AND tag_id=?2 AND source_kind=?3",
        params![asset_path, tag_id, source_kind],
    )?;
    Ok(())
}

/// Drops every reason recorded for one source, whatever tag it named.
///
/// A person identity claims one tag at a time, so withdrawing its claim means
/// removing every row it wrote for this asset, not one tag's worth.
pub fn delete_sources_of(
    connection: &Connection,
    asset_path: &str,
    source_kind: &str,
    source_id: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "DELETE FROM asset_tag_sources WHERE asset_path=?1 AND source_kind=?2 AND source_id=?3",
        params![asset_path, source_kind, source_id],
    )?;
    Ok(())
}

/// Drops the reasons of several kinds at once.
pub fn delete_sources_of_kinds(
    connection: &Connection,
    asset_path: &str,
    tag_id: CustomTagId,
    source_kinds: &[&str],
) -> Result<(), StoreError> {
    for source_kind in source_kinds {
        delete_source_of_kind(connection, asset_path, tag_id, source_kind)?;
    }
    Ok(())
}

/// A hand assignment: drop the legacy inference, record the manual source, or
/// drop both when the user removes the tag by hand.
pub fn replace_manual_source(
    connection: &Connection,
    asset_path: &str,
    tag_id: CustomTagId,
    assigned: bool,
) -> Result<(), StoreError> {
    if assigned {
        delete_source_of_kind(connection, asset_path, tag_id, "legacy")?;
        insert_source(connection, asset_path, tag_id, "manual", "")
    } else {
        delete_sources_of_kinds(connection, asset_path, tag_id, &["manual", "legacy"])
    }
}

/// The reasons recorded for one pair, in a stable order.
pub fn source_kinds(
    connection: &Connection,
    asset_path: &str,
    tag_id: CustomTagId,
) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT source_kind FROM asset_tag_sources WHERE asset_path=?1 AND tag_id=?2 ORDER BY source_kind",
    )?;
    Ok(statement
        .query_map(params![asset_path, tag_id], |row| {
            row.get::<_, String>("source_kind")
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

/// The whole vocabulary, ordered by parent then position, with each tag's
/// materialized `parent|parent|name` path filled in.
pub fn list_tags(connection: &Connection) -> Result<Vec<CustomTag>, StoreError> {
    let rows = {
        let mut statement = connection.prepare(
            "SELECT id, parent_id, name, sort_order
             FROM custom_tags ORDER BY parent_id, sort_order, name COLLATE NOCASE, id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, CustomTagId>("id")?,
                    row.get::<_, Option<CustomTagId>>("parent_id")?,
                    row.get::<_, String>("name")?,
                    row.get::<_, i64>("sort_order")?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let names = rows
        .iter()
        .map(|(id, parent_id, name, _)| (*id, (*parent_id, name.clone())))
        .collect::<HashMap<_, _>>();
    Ok(rows
        .into_iter()
        .map(|(id, parent_id, name, sort_order)| CustomTag {
            id,
            parent_id,
            path: tag_path(id, &names),
            name,
            sort_order,
        })
        .collect())
}

/// The next position among one parent's children.
pub fn next_sort_order(
    connection: &Connection,
    parent_id: Option<CustomTagId>,
) -> Result<i64, StoreError> {
    Ok(connection.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 AS sort_order FROM custom_tags
         WHERE parent_id IS ?1",
        params![parent_id],
        |row| row.get::<_, i64>("sort_order"),
    )?)
}

/// The next position among one parent's children, ignoring the tag being moved
/// into that parent.
pub fn next_sort_order_excluding(
    connection: &Connection,
    parent_id: Option<CustomTagId>,
    excluded: CustomTagId,
) -> Result<i64, StoreError> {
    Ok(connection.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 AS sort_order FROM custom_tags
         WHERE parent_id IS ?1 AND id != ?2",
        params![parent_id, excluded],
        |row| row.get::<_, i64>("sort_order"),
    )?)
}

/// Inserts a tag at the next position and returns its id.
pub fn insert_tag(
    connection: &Connection,
    parent_id: Option<CustomTagId>,
    name: &str,
    name_key: &str,
    sort_order: i64,
) -> Result<CustomTagId, StoreError> {
    connection.execute(
        "INSERT INTO custom_tags(parent_id, name, name_key, sort_order)
         VALUES (?1, ?2, ?3, ?4)",
        params![parent_id, name, name_key, sort_order],
    )?;
    Ok(connection.last_insert_rowid())
}

/// A tag's current parent and position.
pub fn tag_position(
    connection: &Connection,
    id: CustomTagId,
) -> Result<Option<(Option<CustomTagId>, i64)>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT parent_id, sort_order FROM custom_tags WHERE id=?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, Option<CustomTagId>>("parent_id")?,
                    row.get::<_, i64>("sort_order")?,
                ))
            },
        )
        .optional()?)
}

/// Whether `candidate` sits in the subtree rooted at `ancestor`.
pub fn subtree_contains(
    connection: &Connection,
    ancestor: CustomTagId,
    candidate: CustomTagId,
) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "WITH RECURSIVE descendants(id) AS (
           SELECT id FROM custom_tags WHERE id = ?1
           UNION ALL SELECT child.id FROM custom_tags child
             JOIN descendants parent ON child.parent_id = parent.id
         ) SELECT EXISTS(SELECT 1 FROM descendants WHERE id = ?2) AS present",
        params![ancestor, candidate],
        |row| row.get::<_, bool>("present"),
    )?)
}

/// Rewrites a tag's parent, name, and position.
pub fn update_tag(
    connection: &Connection,
    id: CustomTagId,
    parent_id: Option<CustomTagId>,
    name: &str,
    name_key: &str,
    sort_order: i64,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE custom_tags SET parent_id=?1, name=?2, name_key=?3,
           sort_order=?4, updated_at=unixepoch() WHERE id=?5",
        params![parent_id, name, name_key, sort_order, id],
    )?;
    Ok(())
}

/// How many tags and assets a delete would take with it.
pub fn delete_impact(
    connection: &Connection,
    id: CustomTagId,
) -> Result<(usize, usize), StoreError> {
    Ok(connection.query_row(
        "WITH RECURSIVE descendants(id) AS (
           SELECT id FROM custom_tags WHERE id = ?1
           UNION ALL SELECT child.id FROM custom_tags child
             JOIN descendants parent ON child.parent_id = parent.id
         ) SELECT COUNT(DISTINCT descendants.id) AS tag_count,
           COUNT(DISTINCT asset_tags.asset_path) AS asset_count
           FROM descendants LEFT JOIN asset_tags ON asset_tags.tag_id = descendants.id",
        params![id],
        |row| {
            Ok((
                row.get::<_, usize>("tag_count")?,
                row.get::<_, usize>("asset_count")?,
            ))
        },
    )?)
}

/// Every asset carrying the tag or one of its descendants.
pub fn descendant_asset_paths(
    connection: &Connection,
    id: CustomTagId,
) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare(
        "WITH RECURSIVE descendants(id) AS (
           SELECT id FROM custom_tags WHERE id = ?1
           UNION ALL SELECT child.id FROM custom_tags child
             JOIN descendants parent ON child.parent_id = parent.id
         ) SELECT DISTINCT asset_path FROM asset_tags
           WHERE tag_id IN (SELECT id FROM descendants)",
    )?;
    Ok(statement
        .query_map(params![id], |row| row.get("asset_path"))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Deletes one tag. Its subtree and assignments cascade.
pub fn delete_tag(connection: &Connection, id: CustomTagId) -> Result<(), StoreError> {
    connection.execute("DELETE FROM custom_tags WHERE id = ?1", params![id])?;
    Ok(())
}

/// The effective assignments of many assets, as `(asset path, tag id)` pairs.
///
/// Read in bounded chunks so a large selection does not build a statement with
/// one placeholder per asset.
pub fn assigned_tag_ids(
    connection: &Connection,
    asset_paths: &[String],
) -> Result<Vec<(String, CustomTagId)>, StoreError> {
    let mut rows = Vec::new();
    for chunk in asset_paths.chunks(512) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let mut statement = connection.prepare(&format!(
            "SELECT asset_path, tag_id FROM asset_tags WHERE asset_path IN ({placeholders})"
        ))?;
        let found = statement.query_map(
            rusqlite::params_from_iter(chunk.iter()),
            |row| {
                Ok((
                    row.get::<_, String>("asset_path")?,
                    row.get::<_, CustomTagId>("tag_id")?,
                ))
            },
        )?;
        for row in found {
            rows.push(row?);
        }
    }
    Ok(rows)
}

/// Assets carrying every selected tag or a descendant, or any of them.
///
/// `any` selects the match mode; the selected ids and the candidate paths are
/// passed as JSON arrays so one statement serves a page of any size.
pub fn assets_matching_tags(
    connection: &Connection,
    selected_json: &str,
    paths_json: &str,
    any: bool,
) -> Result<HashSet<String>, StoreError> {
    let mut statement = connection.prepare(
        "WITH RECURSIVE selected(id) AS (
            SELECT DISTINCT value FROM json_each(?1)
         ), subtree(root, id) AS (
            SELECT id, id FROM selected
            UNION
            SELECT subtree.root, tag.id FROM custom_tags tag
            JOIN subtree ON tag.parent_id = subtree.id
         )
         SELECT assignment.asset_path FROM asset_tags assignment
         JOIN subtree ON assignment.tag_id = subtree.id
         WHERE assignment.asset_path IN (SELECT value FROM json_each(?2))
         GROUP BY assignment.asset_path
         HAVING ?3 OR COUNT(DISTINCT subtree.root) = (SELECT COUNT(*) FROM selected)",
    )?;
    Ok(statement
        .query_map(params![selected_json, paths_json, any], |row| {
            row.get::<_, String>("asset_path")
        })?
        .collect::<Result<HashSet<_>, _>>()?)
}

/// Assets the XMP mirror still owes a write to, oldest request first.
pub fn pending_sync_paths(connection: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT asset_path FROM tag_xmp_sync_queue ORDER BY requested_at, asset_path",
    )?;
    Ok(statement
        .query_map([], |row| row.get::<_, String>("asset_path"))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Whether this asset is waiting on a mirror write.
pub fn has_pending_sync(connection: &Connection, asset_path: &str) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM tag_xmp_sync_queue WHERE asset_path = ?1) AS present",
        params![asset_path],
        |row| row.get::<_, bool>("present"),
    )?)
}

/// The materialized `parent|parent|name` path of every tag assigned to an
/// asset, ordered by tag id.
pub fn assigned_tag_paths(
    connection: &Connection,
    asset_path: &str,
) -> Result<Vec<String>, StoreError> {
    let tags = {
        let mut statement = connection.prepare(
            "SELECT tag.id, tag.parent_id, tag.name FROM custom_tags tag
             JOIN asset_tags assignment ON assignment.tag_id=tag.id
             WHERE assignment.asset_path=?1 ORDER BY tag.id",
        )?;
        statement
            .query_map(params![asset_path], |row| {
                Ok((
                    row.get::<_, CustomTagId>("id")?,
                    row.get::<_, Option<CustomTagId>>("parent_id")?,
                    row.get::<_, String>("name")?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let all_names = {
        let mut statement = connection.prepare("SELECT id, parent_id, name FROM custom_tags")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, CustomTagId>("id")?,
                    (
                        row.get::<_, Option<CustomTagId>>("parent_id")?,
                        row.get::<_, String>("name")?,
                    ),
                ))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?
    };
    Ok(tags
        .into_iter()
        .map(|(id, _, _)| tag_path(id, &all_names))
        .collect())
}

/// The last mirrored keyword payload for an asset, still as JSON.
///
/// Left as text because what the payload means is the tag domain's business;
/// this function only knows the row.
pub fn xmp_state(
    connection: &Connection,
    asset_path: &str,
) -> Result<Option<(String, String)>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT subjects_json, hierarchical_json FROM asset_tag_xmp_state
             WHERE asset_path = ?1",
            params![asset_path],
            |row| {
                Ok((
                    row.get::<_, String>("subjects_json")?,
                    row.get::<_, String>("hierarchical_json")?,
                ))
            },
        )
        .optional()?)
}

/// Records what was written to the sidecar, as the JSON payloads that were
/// written.
pub fn put_xmp_state(
    connection: &Connection,
    asset_path: &str,
    subjects_json: &str,
    hierarchical_json: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO asset_tag_xmp_state(asset_path, subjects_json, hierarchical_json, synced_at)
         VALUES (?1, ?2, ?3, unixepoch())
         ON CONFLICT(asset_path) DO UPDATE SET subjects_json=excluded.subjects_json,
           hierarchical_json=excluded.hierarchical_json, synced_at=excluded.synced_at",
        params![asset_path, subjects_json, hierarchical_json],
    )?;
    Ok(())
}

/// Clears one asset's pending mirror write.
pub fn delete_sync_entry(connection: &Connection, asset_path: &str) -> Result<(), StoreError> {
    connection.execute(
        "DELETE FROM tag_xmp_sync_queue WHERE asset_path = ?1",
        params![asset_path],
    )?;
    Ok(())
}

/// Records a failed mirror write against one asset.
pub fn record_sync_failure(
    connection: &Connection,
    asset_path: &str,
    error: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE tag_xmp_sync_queue SET attempt_count=attempt_count + 1, last_error=?2
         WHERE asset_path=?1",
        params![asset_path, error],
    )?;
    Ok(())
}

/// How many mirror writes are pending, how many failed, and the newest failure.
pub fn sync_status(
    connection: &Connection,
) -> Result<(usize, usize, Option<String>), StoreError> {
    let (pending_count, failed_count) = connection.query_row(
        "SELECT COUNT(*) AS pending_count, COUNT(last_error) AS failed_count FROM tag_xmp_sync_queue",
        [],
        |row| Ok((row.get("pending_count")?, row.get("failed_count")?)),
    )?;
    let last_error = connection
        .query_row(
            "SELECT last_error FROM tag_xmp_sync_queue WHERE last_error IS NOT NULL
             ORDER BY requested_at DESC LIMIT 1",
            [],
            |row| row.get("last_error"),
        )
        .optional()?;
    Ok((pending_count, failed_count, last_error))
}

/// The id of the child of `parent_id` whose normalized name matches, if any.
///
/// `parent_id` of `None` means a root tag, which is why the comparison is
/// `IS` rather than `=`.
pub fn tag_id_at(
    connection: &Connection,
    parent_id: Option<CustomTagId>,
    name_key: &str,
) -> Result<Option<CustomTagId>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT id FROM custom_tags WHERE parent_id IS ?1 AND name_key = ?2",
            params![parent_id, name_key],
            |row| row.get::<_, CustomTagId>("id"),
        )
        .optional()?)
}

/// The effective assignments of one asset as a set.
pub fn effective_tag_ids(
    connection: &Connection,
    asset_path: &str,
) -> Result<HashSet<CustomTagId>, StoreError> {
    let mut statement = connection.prepare("SELECT tag_id FROM asset_tags WHERE asset_path=?1")?;
    Ok(statement
        .query_map(params![asset_path], |row| {
            row.get::<_, CustomTagId>("tag_id")
        })?
        .collect::<Result<HashSet<_>, _>>()?)
}

/// Copies every recorded source from one asset to another.
///
/// The person source is only carried when `include_person` is set: a file that
/// merely moved inside its folder still shows the same person, while a copy
/// elsewhere is a different asset and must be reviewed on its own.
pub fn copy_source_rows(
    connection: &Connection,
    source: &str,
    destination: &str,
    include_person: bool,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT OR IGNORE INTO asset_tag_sources(asset_path,tag_id,source_kind,source_id)
         SELECT ?2,tag_id,source_kind,source_id FROM asset_tag_sources
         WHERE asset_path=?1 AND (?3=1 OR source_kind!='person')",
        params![source, destination, include_person],
    )?;
    Ok(())
}

/// Copies the effective assignments from one asset to another, under the same
/// person-source rule as [`copy_source_rows`].
pub fn copy_effective_rows(
    connection: &Connection,
    source: &str,
    destination: &str,
    include_person: bool,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT OR IGNORE INTO asset_tags(asset_path, tag_id)
         SELECT DISTINCT ?2,tag_id FROM asset_tag_sources
         WHERE asset_path=?1 AND (?3=1 OR source_kind!='person')",
        params![source, destination, include_person],
    )?;
    Ok(())
}

/// Moves the mirrored keyword state to a new path, replacing anything there.
pub fn move_xmp_state(
    connection: &Connection,
    source: &str,
    destination: &str,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE OR REPLACE asset_tag_xmp_state SET asset_path=?2 WHERE asset_path=?1",
        params![source, destination],
    )?;
    Ok(())
}

/// Drops every tag row belonging to one asset: its recorded sources, its
/// effective assignments, its mirrored keyword state, and its pending write.
pub fn forget_asset_rows(connection: &Connection, asset_path: &str) -> Result<(), StoreError> {
    connection.execute(
        "DELETE FROM asset_tag_sources WHERE asset_path=?1",
        params![asset_path],
    )?;
    connection.execute("DELETE FROM asset_tags WHERE asset_path=?1", params![asset_path])?;
    connection.execute(
        "DELETE FROM asset_tag_xmp_state WHERE asset_path=?1",
        params![asset_path],
    )?;
    delete_sync_entry(connection, asset_path)
}

/// Materializes a tag's path by walking its ancestors.
fn tag_path(
    id: CustomTagId,
    names: &HashMap<CustomTagId, (Option<CustomTagId>, String)>,
) -> String {
    let mut parts = Vec::new();
    let mut current = Some(id);
    while let Some(id) = current {
        let Some((parent, name)) = names.get(&id) else {
            break;
        };
        parts.push(name.as_str());
        current = *parent;
    }
    parts.reverse();
    parts.join("|")
}
