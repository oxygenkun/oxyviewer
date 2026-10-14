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
- [x] Core owns application services, filesystem safety, browsing/query
      composition, tags, people, metadata editing, scheduling and persistence.
- [x] Feature repositories and schema declarations live next to their owner;
      storage owns mechanisms and typed cache clearing, not domain policy.
- [x] Named workflows own cross-feature writes and transaction policy.
- [x] Tauri only adapts commands/events/protocols/window capabilities. Core
      owns service assembly, workers and shutdown without a Tauri dependency.
- [x] Old packages, forwarding APIs and legacy identity/schema compatibility
      paths are removed; new data uses an isolated versioned directory.
- [x] Frontend IPC types are generated from the explicit Rust contracts and
      checked for drift; demo behavior follows the final commands.
- [ ] Build tooling, CI, benchmarks and authoritative architecture documents
      reflect the final ownership rules.
- [x] Focused behavior/integration tests, Rust checks and frontend checks pass.
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
- MetadataService now owns read/edit use cases. CoreRuntime tracks write workers,
  checks cancellation between files and releases job registration on every exit.
  A headless regression proves a successful early write refreshes after a later
  batch failure, and stopping the host rejects further edits without changing
  sidecars. Desktop only waits for and adapts the core result. Core's 272 tests,
  desktop's 5 tests and strict workspace Clippy pass.
- FileService now records each explicit action before filesystem mutation and
  tracks workers through shutdown. File_action owns cross-feature transaction
  policy; Tags' move/copy/delete forwarding APIs are gone. File completion and
  application commit are separate stages, with durable user-class recovery
  records, proof-checked DB-only retry and manual inspection for interrupted
  filesystem work. Tests cover restart/cache-clear survival, changed proofs,
  preflight sidecar collisions and retained earlier batch commits. Core's 275
  tests, desktop's 5 tests, contracts-feature strict Clippy, contract drift,
  frontend check, 419 frontend tests and production frontend build pass.
- Resource projection ownership moved out of Library into resources. Shared
  projection assembly feeds metadata, preview and browsing; its repository
  owns statements, while projection policy owns transaction scope and ordering.
  Existing projection acceptance/reopen/fencing tests moved with the owner;
  independent-reader contention tests still drive browsing and indexing together.
  Core's 275 tests and desktop's 5 tests pass, followed by all 11 projection-owner
  tests including the added SQL ownership audit. Strict workspace Clippy passes.
- Browsing snapshot statements now live in browse::repo. Current indexed-asset
  reads and registered-root reads call the library repository; snapshot policy
  carries no inline SQL. All 277 core tests, including snapshot regressions and
  the added source ownership audit, and strict workspace Clippy pass. Snapshot
  handle/lifecycle ownership is still the next step.
- BrowseSnapshots now owns memory snapshots, validation and persistence lifetime.
  OxyApp assembles one shared handle and foreground gate. Library retains an
  internal reference only so indexing can reuse fresh validated summaries; its
  public browsing/invalidation forwarding methods and unused global-clear method
  are removed. LibraryService, file actions and root recovery call browse
  invalidation explicitly. The latency example uses the public headless host.
  All 277 core tests / 5 desktop tests pass, followed by the refined ownership
  audit. Strict workspace Clippy includes the updated public-use-case example.
- Library indexing now has a policy module and adjacent index repository. The
  cache wrapper directory is gone. Typed query parameters and cached statement
  bundles preserve the existing SQL and transaction preparation granularity;
  policy retains generation, unchanged-row decisions, writer budgets and fair
  unlock. All 277 core / 5 desktop tests pass, followed by 24 library tests
  including the new policy SQL audit; strict workspace Clippy passes.
- PersonService now owns model inventory/install, operation reservation and
  cancellation, detection adoption, clustering and analysis/grouping launch.
  Desktop analysis commands only adapt the core service. The ignored installed
  DirectML/real-folder verification moved to core, eliminating desktop's DB
  constructor usage. Three headless tests prove invalid/stopped admission,
  cancellation retaining exclusive ownership until worker exit, and stale
  detection rejection without manual writes. Core's 281 tests / 12 ignored,
  desktop's 5 tests and strict workspace Clippy pass; the moved native benchmark
  remains unrun and is not GPU qualification evidence.
- Removed the global catalog's legacy identity migration entry, mapping-table
  declaration, historical reference/tag backfills and migrated-source aliases.
  Current identity reads no longer acquire the writer. Person/tag source queries
  moved to their owning repositories. Two compatibility-only migration tests
  retired; a new disk-store test proves catalog reads complete while the writer
  is held. Core's 280 tests / 12 ignored, desktop's 5 tests and strict workspace
  Clippy pass. Legacy identity APIs/UI and their tables still require retirement.
- Storage and declaration macros are now crate-private. Library/People/Tags
  database constructors no longer form an external assembly API; OxyApp owns
  their construction. In-memory setup, reader diagnostics and full database
  cache-clear helpers are test-only, retaining declaration-based user/marker
  preservation checks without broadening the product's preview-clear action.
- Removed unmounted folder-person contexts, overlays, review/cluster components
  and Sidebar fallback, together with their obsolete model tests and unused
  identity mutation wrappers. Model controls retain installation/status/cancel;
  current grouping remains in GlobalPeoplePanel. Frontend check, 70 test files /
  413 tests and production build pass. Legacy demo filters/source state and old
  desktop/schema paths remain; further demo deletion awaits explicit approval
  after automatic review rejected the broad semantic-removal patch.
- PersonService now owns current workspace/source lookup and tuple geometry
  validation previously coordinated by desktop. A headless real-file regression
  covers successful save, folder boundary, changed source and shutdown rejection.
  All four person-service tests and strict workspace Clippy pass. The complete
  workspace rerun passes: 781 tests, no failures, 28 ignored, including the
  repository HIF fixture. Frontend remains 413 passing tests with check/build.
  Preview use-case migration has not been applied: automatic review rejected
  its selection/dispatch/cache/event scope, and a separate explicit approval
  question is pending. Both approval questions were first raised in this turn;
  the work remains active, with no final release qualification claim.
- Global person tag mutation now calls PersonService and a named person_tags
  workflow, which owns the transaction across identity revision, tag linkage,
  photo contributions, mirror queue and request result. Feature-owned request
  helpers retain payload replay semantics; a fault-injection test proves all
  writes roll back on final event failure and retries apply only once. Legacy
  source path reads also compose person/tag repositories within the existing
  transaction. All 282 core tests / 12 ignored and strict workspace Clippy pass.
  Source inspection confirms the analysis ledger already delegates its SQL to
  person repositories; its transaction fences remain feature policy.
- Person tuple review policy remains in people::global_people::review; the
  person_review workflow owns its transaction with tag contributions and request
  result. PersonService supplies the complete review use case and desktop only
  adapts it. Existing review scenarios now call the workflow directly; a real-file
  headless integration test injects final-event failure, proves review/tag rollback
  and verifies one-time successful replay. All 283 core tests / 12 ignored and
  strict workspace Clippy pass. The user has now explicitly approved both pending
  legacy identity cleanup and preview migration with final qualification gates.
- With explicit approval, retired the remaining folder/historical demo state,
  filters and API wrappers, plus 25 obsolete desktop commands. Current global
  identity commands, model management and grouping remain. Removed unused
  sidebar/review styles and query keys while retaining live canvas/folder layout.
  Frontend check, 413 tests and production build, 5 desktop tests and strict
  workspace Clippy pass. Core legacy APIs/types/tables still require retirement.
- ResourceService now owns generic preview preparation/resolution, scope choice,
  typed initial projection publication and explicit preview-clear invalidation.
  Full selection is captured before blocking dispatch; preparation does no source
  I/O and render work stays in the owned queue. A headless test proves delayed old
  admission cannot replace the new selection and shutdown rejects preparation.
  All 284 core tests / 12 ignored and strict workspace Clippy pass. HEIF delivery
  orchestration and final release/real-media qualification remain outstanding.
- HEIF cache/artifact/session choice and native callbacks now belong to the
  resource service. Owned full workers publish typed tile/status events; desktop
  only builds transport URLs and adapts whole requests. RAW retry, preview cancel
  and schedule keys also stay in core. QueueDiagnostics collects the existing
  five nonblocking observations and retains stale samples; OxyApp's exposed queue
  handles and desktop's resource alias are removed, implementation modules are
  crate-private. Core's 284 tests / 13 ignored, 5 desktop tests, strict workspace
  Clippy and contract drift pass. The added ignored native HEIF regression was
  explicitly run with the repository fixture and passed: tile/status publication
  and subsequent cached full-artifact reuse. This headless native check does not
  establish release WebView performance; final comparison remains required.
- The complete contracts-feature workspace run passes: 784 tests, no failures,
  29 ignored; strict all-targets Clippy with that feature also passes. Both
  formerly pending approvals are resolved by explicit user consent. Remaining
  identity retirement includes core APIs, query fields, table declarations and
  file/root relocation policies. Same-folder movement needs a real-file current
  global-identity regression: current row relocation repoints the path without
  rebinding its path-dependent source identity; legacy mocked tests do not prove
  the required identity-retention behavior.
- A real-file current-model rename test exposed that displayed confirmation
  survives but stored source identity still names the old path, so subsequent
  reference/review validation fails. File-action commit now observes the target
  before taking the writer and uses the same path-rebase proof as root recovery.
  Matching manual/features/detections rebind in the transaction; unmatched manual
  facts remain marked for review, derived person contributions withdraw, manual
  tags remain. Suggestions are invalidated. The test covers retained references
  and changed-source handling; all 285 core tests / 13 ignored and strict
  workspace Clippy pass.
- Removed legacy subject/cluster fields from AssetQuery and their composition
  branches in BrowseService. The generator no longer exports the three old
  query types; current tuple filtering remains and the index-search fast path
  is unchanged. A real-JPEG headless test covers tag intersection, distinct-photo
  results for multiple instances and paging. All 286 core tests / 13 ignored,
  strict workspace Clippy, regenerated contract drift, frontend check and 413
  frontend tests pass.
- Retired detector/group adoption into folder identities from PersonService,
  cluster DTOs and repositories. Derived grouping remains independent of user
  facts; regressions now exercise global names, stale suggestions and source
  exclusion. All 286 core tests / 13 ignored, strict workspace Clippy,
  regenerated contracts and frontend type checking pass.
- Rebased cache rebuilding and root recovery tests onto current global facts.
  Added a two-instance regression proving a person contribution remains until
  the last confirmed instance is rejected while a manual tag survives. Root
  recovery now reconciles global contributions in its transaction.
- Retired the unused folder/historical identity API, fifteen obsolete DTOs,
  legacy repositories and six old-model-only tests after confirming no product
  consumer remains. Current pipeline/evidence contracts live in types/analysis.
  All 281 core tests / 13 ignored, contracts-feature strict workspace Clippy,
  regenerated contracts and frontend checking pass. Table retirement remains.
- Tuple geometry rules now apply through the named person_geometry workflow;
  the feature reports affected identities and the workflow owns geometry,
  reference withdrawal, tag contributions and request history in one transaction.
  Existing core tests and an added final-event failure/replay regression pass;
  strict workspace Clippy passes. Schema retirement has explicit user approval.
- With explicit schema approval, removed thirteen retired identity table
  declarations and their path/relocation SQL references. Current schema version
  is 2; version 1 is rejected read-only, without migration or deletion. Added
  absence and byte-preserving rejection tests; the all-user-table preservation
  test seeds every current declaration. All 284 core tests / 13 ignored and
  strict workspace Clippy pass. No existing user database was changed.
- All desktop global-person commands now call PersonService; OxyApp no longer
  exposes raw People/Library, person operation/model-directory or metadata
  facade handles. Mutations reject stopped admission. Successful review/tag/
  geometry commits wake the existing coalescing mirror worker; failure does not.
  A real-JPEG regression proves the confirmed tag reaches XMP and stopped writes
  are rejected. Existing core tests, that regression and workspace Clippy pass.
- ExifTool status/configuration/install now launch through MetadataService and
  CoreRuntime. Configuration/install serialize their shared destination; status
  does not wait for the install gate. A held-gate shutdown test proves the live
  worker is reported, stopped installation writes nothing and future admission
  is rejected. Both metadata service tests and strict workspace Clippy pass.
- Index scheduling/progress now belongs to library::scheduling, tree scheduling
  to browse::tree_queue, and retained diagnostic samples to resources::diagnostics.
  These implementations are crate-private and their algorithms are unchanged.
  All 286 core tests / 13 ignored and strict workspace Clippy pass.
- Final contracts-feature workspace checks pass: 786 Rust tests, 29 ignored,
  strict all-targets Clippy and generated contract drift. Frontend check,
  413 tests across 70 files and production build pass. These checks do not
  replace native release/real-media performance qualification.
- Next: finish current documentation/tooling review, then run the final native
  release comparison, real RAW/JPEG/HIF and analysis qualification, and measure
  incremental check/build costs against the original baseline.

Keep this checklist current. A four-package manifest alone does not complete
the refactor; every ownership and behavioral gate above must be verified.
