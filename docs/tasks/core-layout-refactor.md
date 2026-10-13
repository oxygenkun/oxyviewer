# Four-package core layout refactor

Status: active. Branch: `codex/core-layout-refactor`.

Implement the [2026-10-13 report](../research/2026-10-13-code-organization-report.md).
The work runs in an isolated managed worktree. The functional baseline is
`595a46b` (repeat-last-tag shortcut); unrelated edits remain in the original
checkout. Old package/API/schema compatibility is not a target. Preserve user
facts, safe paths, cheap paged folder opening, independent review decisions,
resource leases, cancellation and stale-publication semantics.

## Required end state

- [x] Workspace contains desktop, oxy-core, oxy-media and oxy-metadata-parser.
- [x] Media owns its types and decode controls, and depends only on the parser
      among workspace packages; parser remains independent.
- [ ] Core owns application services, filesystem safety, browsing/query
      composition, tags, people, metadata editing, scheduling and persistence.
- [ ] Feature repositories and schema declarations live next to their owner;
      storage owns mechanisms and typed cache clearing, not domain policy.
- [ ] Named workflows own cross-feature writes and transaction policy.
- [ ] Tauri only adapts commands/events/protocols/window capabilities. Core
      owns service assembly, workers and shutdown without a Tauri dependency.
- [ ] Old packages, forwarding APIs and legacy identity/schema compatibility
      paths are removed; new data uses an isolated versioned directory.
- [x] Frontend IPC types are generated from the explicit Rust contracts and
      checked for drift; demo behavior follows the final commands.
- [ ] Build tooling, CI, benchmarks and authoritative architecture documents
      reflect the final ownership rules.
- [ ] Focused behavior/integration tests, Rust checks and frontend checks pass.
- [ ] Release WebView and real media performance are compared with the baseline;
      incremental build costs are measured for feature/type/media edits.

## Evidence and progress

- Created the branch and managed worktree; original checkout stays on `dev`.
- Reviewed and committed the cohesive existing tag shortcut feature as
  `595a46b`, then fast-forwarded the refactor branch to it.
- Baseline frontend: check, 73 test files / 419 tests, and build passed.
  Source hashes match the validated files. Browser demo confirmed tag repeat
  on a different photo, removal on the next press and mixed-selection toggles.
- The first Rust baseline ran in the restricted Windows environment and hit
  filesystem permission failures (including doctest executable creation).
  The permitted worktree rerun passed: 766 tests, no failures, 28 ignored,
  including the real HIF fixture and executable doctests.
- First implementation step moves media-owned types, cooperative controls and
  source observations into oxy-media, removes its domain/fs/runtime package
  dependencies, and pulls application relocation policy into the library
  application module. All 766 existing tests passed after the ownership move;
  strict workspace Clippy and media bench-tools checks passed. The additional
  application recovery regression passed with the 54 library tests.
  Old application packages remain until their behavior has migrated into core.
- Built the original `595a46b` release through the repository Tauri wrapper and
  recorded three native WebView HIF baseline scenarios in isolated state:
  cold preview (3 runs), 1,100-path thumbnail retention (1 run), and 80-path
  filmstrip scroll (3 runs). The fixture is one real HIF under repeated paths;
  this does not establish different-photo/NAS throughput or clear OS caches.
- Application package convergence is implemented: eight old packages were
  removed and their behavior moved into core modules; desktop call sites and
  the library benchmark now use core paths. Workspace check, strict Clippy,
  formatting and 763 tests passed, with 28 ignored. The count reflects one
  added recovery regression and retirement of four obsolete package-boundary
  source audits; data-class and cache/user behavior audits remain.
- Reader-only metadata now lives in media. Core has no direct parser or
  libheif dependency; sidecar precedence and capture/focus regressions pass.
- OxyApp now assembles services and one shared store without Tauri. Core owns
  resource queues, maintenance and person worker admission. Typed events are
  forwarded by a joined desktop adapter. Shutdown rejects admission/events,
  cancels decode/session work and person operations, releases waiters, wakes
  foreground gates and stops snapshot persistence. Native work that cannot
  unwind within the deadline reports a timeout rather than fake completion.
- Metadata admission serializes only the same request; SQLite work is outside
  the queue lock, with directory/generation/stop checks before insertion.
  A blocked-writer regression verifies diagnostics and stop remain available.
- Validation: full workspace tests passed (769 tests / 28 ignored), followed
  by core's two added lifecycle/admission regressions (270 passed / 11 ignored).
  Strict workspace Clippy and frontend check pass; frontend tests remain
  73 files / 419 passing. A media handoff test's fixed sleep was replaced by
  waiting for the actual subscriber after its parallel-run failure reproduced.
- Application types moved to feature types modules; contracts is now an export
  facade, and core implementations import their owner directly. Existing
  repositories moved next to library/tags/people; the central storage repo
  entrance was removed. Named app workflows now hold asset moves/deletions,
  person tag contributions and root recovery; root row writes were separated
  into their owning feature repositories. All 48 table declarations now sit in
  feature schemas; storage registers their classes without guessing from names.
  Strict workspace Clippy and 270 core tests pass after these moves.
- Pinned ts-rs 12.0.1 exports 155 serialized types and four aliases to the
  checked TypeScript artifact. Frontend-only URL, tree-root, query-control and
  paging projections derive their fields from those definitions. Browser demo
  output and nullable consumers now match Rust; metadata null normalization
  preserves unchanged visible asset identity. CI checks contract drift and
  includes the optional generator in Clippy. Full workspace tests with the
  contracts feature pass: 771 tests / 28 ignored; frontend check, 419 tests and
  production build pass. Cross-platform CI and final native performance remain
  qualification gates, not claims established by these Windows checks.
- Eight browsing use cases moved out of desktop commands into BrowseService:
  folder open, paging and multi-feature query composition, directory-tree
  operations, active-directory cancellation, search and refresh invalidation.
  Snapshot validation is runtime-owned and browse progress is a typed event.
  Tag mirror helpers and their test moved to core; publication bursts coalesce
  into one owned worker and failed syncs remain durable for retry. Core and
  desktop tests passed (273 / 5), followed by the added worker-lifetime test.
  Headless tests prove nonrecursive paging and tag/metadata filter intersection.
  Strict workspace Clippy and frontend check pass for the migration.
- Default bootstrap and custom cache paths are isolated under core-v2. Current
  schema creation is transactional and tagged with application ID/version;
  existing files undergo read-only preflight so rejecting an old/future file
  cannot checkpoint or rewrite it. Feature schemas own indexes, FTS and format
  invalidation. Retired old column upgrades, root/tag backfills and flat-cache
  deletion, together with seven compatibility-only tests. Three new tests prove
  old/unknown files and old data/cache locations remain untouched. Core has
  270 passing tests / 11 ignored; strict workspace Clippy, frontend check and
  419 frontend tests pass. Snapshot
  workers drain accepted writes before exit so cached empty directories survive.
- LibraryService owns root configuration, cache proof gathering, confirmed
  recovery, artifact reuse and deferred indexing. Root recovery transactions
  now belong to workflow functions rather than methods forwarded by Library.
  Desktop commands only offload and adapt results. Existing rollback/stale-plan
  tests and a headless service regression pass (271 core / 5 desktop tests).
- Next: distribute browsing SQL, move remaining desktop use-case coordination,
  remove legacy person identity paths and finish ownership/docs/tooling gates,
  then complete final release/performance qualification.

Keep this checklist current. A four-package manifest alone does not complete
the refactor; every ownership and behavioral gate above must be verified.
