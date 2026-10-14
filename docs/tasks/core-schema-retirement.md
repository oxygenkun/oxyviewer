# Global identity schema retirement

Status: implemented with explicit user approval; 284 core tests and strict workspace Clippy pass.

The accepted refactor removes old folder/historical identities. Their product
APIs and Rust implementations are retired in `996cfd8`; current global behavior
is covered by cache rebuilding, root recovery, rename, reference, independent
review, idempotency and tag-source regressions. Source table declarations remain.

## Proposed concrete change

Remove these declarations from `people/schema.rs` for newly created databases:

| Old table | Current owner or replacement |
| --- | --- |
| folder_people | global_people |
| person_cluster_adoptions | derived suggestions and explicit tuple review |
| person_review_decisions | global_person_reviews |
| person_review_events | global_person_events |
| person_identity_events | global_person_events |
| person_references | global_person_references |
| person_instance_events | geometry before/result in global_person_events |
| person_request_results | payload/result in global_person_events |
| historical_people | global_people and confirmed galleries |
| folder_historical_links | global_person_reviews |
| person_history_events | global_person_events |
| person_tag_links | global_person_tags |
| person_tag_overrides | explicit tuple decisions and independent tag sources |

Retain `person_manual_instances` and all current global user declarations.
Remove old-table references from root path projections and person relocation
statements. Update the schema preservation fixture to seed every current user
table and keep the declaration/class coverage test.

Change the core schema version from 1 to 2. Keep the isolated `core-v2` namespace.
An interim refactor database tagged version 1 will be rejected before opening a
writer; it will not be upgraded, reset, checkpointed or deleted. A new regression
must prove rejection leaves its bytes and user rows intact. Original pre-refactor
data lives outside this namespace and remains untouched.

No DROP TABLE, DELETE against user facts, cache-clear expansion, database-file
removal or automatic migration is proposed. Anyone who already used this branch
with schema 1 must retain that database separately and use a fresh test directory
for schema 2. Import or migration of those interim facts is outside this change.

## Verification required

- Every created table has one declared class; no old identity table is created.
- Cache clearing preserves rows in every current user table and marker.
- Version-1 rejection is read-only and leaves stored user facts intact.
- Core/workspace checks, current root/file recovery, review/tag transactions,
  generated contracts and frontend tests pass.

Automatic review rejected the combined source/schema script and later the
schema-only script because table retirement and version incompatibility were
considered high impact. Neither rejected script executed.
