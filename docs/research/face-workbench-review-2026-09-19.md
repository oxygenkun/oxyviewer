# Unified face workbench verification — 2026-09-19

The photo review surface now shares cards and face-level actions across default,
manual-status and automatic-cluster grouping. The existing encoded thumbnail
request is reused; region overlays use display-normalized geometry.

## Verification

- Frontend type check, 377 tests across 68 Vitest files, and production frontend build.
- macOS Release build via `pnpm tauri build --no-bundle`.
- Browser UI: select all five demo observations, confirm from one card, and
  verify the five photos move into the named-person column with updated labels.
- Native E2E: `node tests/perf/perf-e2e.mjs --scenario faces-workbench --runs 1
  --app target/release/oxyviewer` using the repository two-person JPEG repeated
  at twelve fixture paths. The runner isolates data and closes the application.

The native probe waits for actual model analysis, renders the production photo
review component, verifies decoded images and overlays, checks selection reset
on view changes, then creates an identity from a selected card and verifies all
24 confirmations through the native review API. Resource high-water marks must
remain within the configured native registry budgets.

An initial pass displayed a 512×301 registered JPEG and face boxes in 55 ms.
The expanded probe including persisted batch assignment displayed them in 85 ms;
its registry peaks were 18 entries and 334,586 encoded bytes. The final build rerun displayed the preview in 51 ms, with 18 peak entries
and 191,192 encoded bytes, and again persisted all 24 confirmations. All passed the
800 ms selected-preview guardrail from PERFORMANCE.md. The reports are under
ignored `tests/perf/.reports/` and subsequent runs replace the same report.

## Limits

Analysis and normal grid warming precede the workbench measurement, so these
are **warm thumbnail** observations, not cold source-decode qualifications.
The repeated fixture verifies geometry, resource ownership and interaction; it
is not a diverse face-accuracy corpus or a 100k-directory benchmark. The known
300 ms large-directory first-paint shortfall remains unchanged.

Review rows are loaded in pages of 200 observations. Filters and groups use the
loaded subset and the UI states that scope. Photos with no detected regions are
not returned by this face-observation data source. Identity assignments use the
existing durable undo journal; pending/non-face batches use per-observation
writes, refresh after partial failures, and do not claim atomic batch undo.
