//! Tables declared by the feature that owns their meaning.
crate::storage::table::tables! {
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
    cache create global_person_suggestions =
        "folder_path TEXT PRIMARY KEY, run_id TEXT NOT NULL, payload TEXT NOT NULL";
    marker create person_detection_cache_meta = "schema_version INTEGER NOT NULL";
    cache create person_instances_cache =
        "folder_path TEXT NOT NULL,
        asset_path TEXT NOT NULL,
        instance_id TEXT NOT NULL,
        source_revision TEXT NOT NULL,
        producer_fingerprint TEXT NOT NULL,
        pipeline_fingerprint TEXT NOT NULL,
        run_id TEXT NOT NULL,
        face_box TEXT,
        face_landmarks TEXT,
        body_box TEXT,
        face_score REAL,
        body_score REAL,
        association_score REAL,
        updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
        PRIMARY KEY(folder_path,asset_path,producer_fingerprint,instance_id)";
    marker create person_vector_cache_meta = "schema_version INTEGER NOT NULL";
    cache create person_feature_spaces =
        "id TEXT PRIMARY KEY,
        modality TEXT NOT NULL CHECK(modality IN ('face','body')),
        dimension INTEGER NOT NULL CHECK(dimension > 0 AND dimension <= 4096),
        producer_fingerprint TEXT NOT NULL,
        format_version INTEGER NOT NULL DEFAULT 1";
    cache create person_features_cache =
        "feature_row_id INTEGER PRIMARY KEY,
        folder_path TEXT NOT NULL,
        asset_path TEXT NOT NULL,
        instance_id TEXT NOT NULL,
        source_revision TEXT NOT NULL,
        feature_space_id TEXT NOT NULL REFERENCES person_feature_spaces(id),
        pipeline_fingerprint TEXT NOT NULL,
        vector BLOB NOT NULL,
        updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
        UNIQUE(folder_path,asset_path,instance_id,feature_space_id)";
    cache create person_analysis_heads = "folder_path TEXT PRIMARY KEY, generation INTEGER NOT NULL, run_id TEXT NOT NULL";
    cache create person_analysis_runs =
        "run_id TEXT PRIMARY KEY,
        folder_path TEXT NOT NULL,
        pipeline_id TEXT NOT NULL,
        pipeline_fingerprint TEXT NOT NULL,
        generation INTEGER NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('queued','running','completed','failed','cancelled')),
        enumeration_complete INTEGER NOT NULL DEFAULT 0,
        total_tasks INTEGER NOT NULL DEFAULT 0,
        completed_tasks INTEGER NOT NULL DEFAULT 0,
        failed_tasks INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch())";
    cache create person_analysis_requests =
        "request_id TEXT PRIMARY KEY,
        operation TEXT NOT NULL,
        run_id TEXT NOT NULL REFERENCES person_analysis_runs(run_id)";
    cache create person_analysis_tasks =
        "run_id TEXT NOT NULL REFERENCES person_analysis_runs(run_id),
        asset_path TEXT NOT NULL,
        source_revision TEXT NOT NULL,
        stage_id TEXT NOT NULL,
        stage_fingerprint TEXT NOT NULL,
        stage_order INTEGER NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('queued','running','completed','failed')),
        claim_token TEXT,
        error TEXT,
        PRIMARY KEY(run_id,asset_path,stage_id),
        UNIQUE(run_id,asset_path,stage_order)";
    cache create person_cluster_snapshots =
        "folder_path TEXT PRIMARY KEY,
        run_id TEXT NOT NULL,
        snapshot_json TEXT NOT NULL";
}
