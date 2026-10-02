//! Tables that hold what the user typed, named, confirmed, or assigned.
//!
//! Nothing here is ever emptied by a cache clear, a cache migration, or a
//! re-index. Changing these rows requires an explicit user action and, for a
//! destructive one, an explicit confirmation.
//!
//! Classes are not uniform because the declared module was: `folder_people`
//! and `person_manual_instances` are identity the user confirmed, while
//! `person_feature_spaces` and `person_features_cache` are derived vectors
//! that live in [`super::cache`]. The two sit a few lines apart in separate
//! files precisely because the class is a word in the declaration.

use super::has_column;
use crate::table;
use rusqlite::{Connection, params};

crate::table::tables! {
    user create library_roots =
        "path TEXT PRIMARY KEY NOT NULL,
        added_at INTEGER NOT NULL DEFAULT (unixepoch()),
        sort_order INTEGER";
    user create custom_tags =
        "id INTEGER PRIMARY KEY AUTOINCREMENT,
        parent_id INTEGER REFERENCES custom_tags(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        name_key TEXT NOT NULL,
        sort_order INTEGER NOT NULL,
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create asset_tags =
        "asset_path TEXT NOT NULL,
        tag_id INTEGER NOT NULL REFERENCES custom_tags(id) ON DELETE CASCADE,
        assigned_at INTEGER NOT NULL DEFAULT (unixepoch()),
        PRIMARY KEY(asset_path, tag_id)";
    user create asset_tag_xmp_state =
        "asset_path TEXT PRIMARY KEY NOT NULL,
        subjects_json TEXT NOT NULL DEFAULT '[]',
        hierarchical_json TEXT NOT NULL DEFAULT '[]',
        synced_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create tag_xmp_sync_queue =
        "asset_path TEXT PRIMARY KEY NOT NULL,
        requested_at INTEGER NOT NULL DEFAULT (unixepoch()),
        attempt_count INTEGER NOT NULL DEFAULT 0,
        last_error TEXT";
    // Created by `ensure_tag_sources` rather than by `create_all`: its backfill
    // has to run in its own transaction, after `asset_tags` exists and holds
    // the assignments the backfill derives sources from.
    user external asset_tag_sources;
    user create folder_people =
        "id TEXT PRIMARY KEY,
        folder_path TEXT NOT NULL,
        display_name TEXT,
        identity_confirmed INTEGER NOT NULL DEFAULT 0,
        revision INTEGER NOT NULL DEFAULT 1,
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create person_cluster_adoptions =
        "folder_path TEXT NOT NULL,
        cluster_id TEXT NOT NULL,
        subject_id TEXT NOT NULL REFERENCES folder_people(id),
        PRIMARY KEY(folder_path,cluster_id,subject_id)";
    user create person_manual_instances =
        "id TEXT PRIMARY KEY,
        folder_path TEXT NOT NULL,
        asset_path TEXT NOT NULL,
        source_revision TEXT NOT NULL,
        source_identity_revision TEXT,
        face_box TEXT,
        body_box TEXT,
        needs_review INTEGER NOT NULL DEFAULT 0,
        revision INTEGER NOT NULL DEFAULT 1,
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create person_review_decisions =
        "instance_id TEXT NOT NULL REFERENCES person_manual_instances(id),
        subject_id TEXT NOT NULL REFERENCES folder_people(id),
        decision TEXT NOT NULL CHECK(decision IN ('pending','belongs','doesNotBelong','deferred')),
        revision INTEGER NOT NULL DEFAULT 1,
        updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
        PRIMARY KEY(instance_id, subject_id)";
    user create person_review_events =
        "id INTEGER PRIMARY KEY AUTOINCREMENT,
        instance_id TEXT NOT NULL,
        subject_id TEXT NOT NULL,
        decision TEXT NOT NULL,
        revision INTEGER NOT NULL,
        request_id TEXT NOT NULL UNIQUE,
        changed_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create person_identity_events =
        "id INTEGER PRIMARY KEY AUTOINCREMENT,
        subject_id TEXT NOT NULL,
        event_kind TEXT NOT NULL,
        display_name TEXT,
        revision INTEGER NOT NULL,
        request_id TEXT NOT NULL UNIQUE,
        changed_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create person_references =
        "subject_id TEXT NOT NULL REFERENCES folder_people(id),
        instance_id TEXT NOT NULL REFERENCES person_manual_instances(id),
        source_revision TEXT NOT NULL,
        confirmed_at INTEGER NOT NULL DEFAULT (unixepoch()),
        PRIMARY KEY(subject_id, instance_id)";
    user create person_instance_events =
        "id INTEGER PRIMARY KEY AUTOINCREMENT,
        instance_id TEXT NOT NULL,
        previous_json TEXT NOT NULL,
        request_id TEXT NOT NULL UNIQUE,
        changed_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create person_request_results = "request_id TEXT PRIMARY KEY, operation TEXT NOT NULL, entity_id TEXT NOT NULL";
    user create historical_people =
        "id TEXT PRIMARY KEY,
        display_name TEXT NOT NULL,
        reference_asset_path TEXT NOT NULL,
        reference_source_revision TEXT NOT NULL,
        revision INTEGER NOT NULL DEFAULT 1,
        created_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create folder_historical_links =
        "subject_id TEXT PRIMARY KEY REFERENCES folder_people(id),
        historical_person_id TEXT NOT NULL REFERENCES historical_people(id),
        linked_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create person_history_events =
        "id INTEGER PRIMARY KEY AUTOINCREMENT,
        subject_id TEXT NOT NULL,
        historical_person_id TEXT NOT NULL,
        event_kind TEXT NOT NULL CHECK(event_kind IN ('link','unlink')),
        request_id TEXT NOT NULL UNIQUE,
        linked_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create person_tag_links =
        "historical_person_id TEXT PRIMARY KEY REFERENCES historical_people(id),
        tag_id INTEGER REFERENCES custom_tags(id) ON DELETE SET NULL,
        enabled INTEGER NOT NULL DEFAULT 0,
        revision INTEGER NOT NULL DEFAULT 1,
        updated_at INTEGER NOT NULL DEFAULT (unixepoch())";
    user create person_tag_overrides =
        "historical_person_id TEXT NOT NULL REFERENCES historical_people(id),
        asset_path TEXT NOT NULL,
        suppressed INTEGER NOT NULL DEFAULT 1,
        revision INTEGER NOT NULL DEFAULT 1,
        PRIMARY KEY(historical_person_id,asset_path)";
    user create global_people =
        "id TEXT PRIMARY KEY, display_name TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 1";
    user create global_person_migrations =
        "subject_id TEXT PRIMARY KEY, person_id TEXT NOT NULL REFERENCES global_people(id)";
    user create global_person_reviews =
        "instance_id TEXT NOT NULL REFERENCES person_manual_instances(id), person_id TEXT NOT NULL REFERENCES global_people(id),
         decision TEXT NOT NULL CHECK(decision IN ('pending','belongs','doesNotBelong','deferred')),
         revision INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(instance_id,person_id)";
    user create global_person_targets =
        "instance_id TEXT PRIMARY KEY REFERENCES person_manual_instances(id), person_id TEXT NOT NULL REFERENCES global_people(id)";
    user create global_person_references =
        "person_id TEXT NOT NULL REFERENCES global_people(id), instance_id TEXT NOT NULL REFERENCES person_manual_instances(id),
         PRIMARY KEY(person_id,instance_id)";
    user create global_person_events =
        "request_id TEXT PRIMARY KEY, payload TEXT NOT NULL, result TEXT NOT NULL";
    user create global_person_tags =
        "person_id TEXT PRIMARY KEY REFERENCES global_people(id), tag_id INTEGER REFERENCES custom_tags(id) ON DELETE SET NULL";

}

/// Creates or migrates every user-owned table.
pub(super) fn ensure_schema(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    table::create_all(connection, DEFS)?;
    connection.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS custom_tags_sibling_name
           ON custom_tags(COALESCE(parent_id, 0), name_key);
         CREATE INDEX IF NOT EXISTS custom_tags_parent
           ON custom_tags(parent_id, sort_order, name);
         CREATE INDEX IF NOT EXISTS asset_tags_tag ON asset_tags(tag_id, asset_path);
         CREATE INDEX IF NOT EXISTS folder_people_folder ON folder_people(folder_path);
         CREATE INDEX IF NOT EXISTS person_manual_instances_asset
           ON person_manual_instances(folder_path, asset_path);
         CREATE INDEX IF NOT EXISTS folder_historical_links_person
           ON folder_historical_links(historical_person_id);",
    )?;
    // Column migrations for files written before the column existed.
    if !has_column(connection, "library_roots", "sort_order")? {
        connection.execute(
            "ALTER TABLE library_roots ADD COLUMN sort_order INTEGER",
            [],
        )?;
    }
    if !has_column(
        connection,
        "person_manual_instances",
        "source_identity_revision",
    )? {
        connection.execute(
            "ALTER TABLE person_manual_instances ADD COLUMN source_identity_revision TEXT",
            [],
        )?;
    }
    normalize_root_order(connection)?;
    ensure_tag_sources(connection)
}

/// Gives every root a dense sort order, so the sidebar keeps the order the user
/// arranged in even after a root is added by an older build that had no column.
fn normalize_root_order(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    let paths = {
        let mut statement = connection.prepare(
            "SELECT path FROM library_roots
             ORDER BY sort_order IS NULL, sort_order, added_at, path",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>("path"))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let transaction = connection.transaction()?;
    for (sort_order, path) in paths.iter().enumerate() {
        transaction.execute(
            "UPDATE library_roots SET sort_order = ?1 WHERE path = ?2",
            params![sort_order as i64, path],
        )?;
    }
    transaction.commit()
}

/// Creates `asset_tag_sources` and derives a `legacy` source for every
/// assignment that predates it.
///
/// This runs in its own transaction because the backfill reads `asset_tags`,
/// which `create_all` above has just created.
fn ensure_tag_sources(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "BEGIN;
         CREATE TABLE IF NOT EXISTS asset_tag_sources (
           asset_path TEXT NOT NULL,
           tag_id INTEGER NOT NULL REFERENCES custom_tags(id) ON DELETE CASCADE,
           source_kind TEXT NOT NULL CHECK(source_kind IN ('legacy','manual','sidecar','person')),
           source_id TEXT NOT NULL DEFAULT '',
           PRIMARY KEY(asset_path,tag_id,source_kind,source_id)
         );
         CREATE INDEX IF NOT EXISTS asset_tag_sources_tag
           ON asset_tag_sources(tag_id,source_kind,asset_path);
         INSERT OR IGNORE INTO asset_tag_sources(asset_path,tag_id,source_kind,source_id)
           SELECT a.asset_path,a.tag_id,'legacy','' FROM asset_tags a
           WHERE NOT EXISTS (SELECT 1 FROM asset_tag_sources s
             WHERE s.asset_path=a.asset_path AND s.tag_id=a.tag_id);
         COMMIT;",
    )
}
