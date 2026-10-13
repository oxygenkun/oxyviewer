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
- [ ] Frontend IPC types are generated from the explicit Rust contracts and
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
- Next: move reader-only metadata into media, distribute contracts/repos/schema
  by feature, introduce named workflows and OxyApp lifecycle, and move desktop
  queues/use-case coordination into core. Package convergence does not yet
  satisfy those ownership and lifecycle gates.

Keep this checklist current. A four-package manifest alone does not complete
the refactor; every ownership and behavioral gate above must be verified.
