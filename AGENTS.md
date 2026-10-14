# AGENTS.md

Repository-specific rules for coding agents working on OxyViewer.

## Start Here

- Use [CONTRIBUTING.md](CONTRIBUTING.md) for setup and the verification matrix.
- Use [docs/README.md](docs/README.md) to find current architecture, format,
  packaging, roadmap, task and research documents.
- Rust changes follow [docs/RUST_STYLE.md](docs/RUST_STYLE.md). Workspace
  packages inherit `[lints] workspace = true`; no crate-wide Clippy exemptions.
- Before changing browsing, caches, queues, scheduling or diagnostics, read
  [docs/PERFORMANCE_INVARIANTS.md](docs/PERFORMANCE_INVARIANTS.md).

## Ownership and Contracts

- The Rust workspace has desktop, `oxy-core`, `oxy-media` and
  `oxy-metadata-parser`. Application features are modules in core. Media owns
  native reads, decode, pixels, artifacts and leases; parser is independent.
  Do not recreate forwarding crates or split features without a concrete reuse,
  dependency-direction or measured build reason.
- Keep `apps/desktop/src-tauri` thin: command/event/protocol/window adaptation.
  `OxyApp` opens one store and assembles services, queues and owned workers.
  Core never imports Tauri or holds an AppHandle/WebView.
- Public serialized types live beside their feature, with camelCase serde
  names, and are explicitly exported by `oxy-core::contracts`. Media types
  belong to media. Generate TypeScript with `pnpm contracts:generate`; verify
  drift with `pnpm contracts:check`. Update `apps/desktop/src/lib/api.ts` and
  browser-demo behavior when commands change.
- Feature rules, repositories and schema declarations stay next to their owner.
  `storage` owns connections, WAL readers, the table macro, registry and version
  mechanism; it is private to core. A repo function opens no transaction and
  decides no domain rule. Read columns by name, never position.
- A single-feature policy may open its own transaction. A cross-feature write
  belongs to a named `app::workflows` function that calls owning repositories
  and transaction-local feature operations. Do not put all SQL in a central
  cross module or make one feature coordinate another feature's user policy.
- Current person identities are global. Folder groups and machine detections
  are suggestions; explicit tuple review establishes human facts. Independent
  people in one photo retain independent decisions, references and revisions.
  Geometry/review/tag workflows commit human changes, tag contributions,
  durable mirror requests and idempotent results together. Wake mirror work
  only after successful commit; preserve manual and sidecar sources.
- Route discovery and safe user file operations through `oxy-core::files`.
  File actions and SQLite cannot form one atomic transaction. Preserve per-file
  recovery records, source proofs, partial successes and DB-only retry; never
  automatically repeat an uncertain filesystem action.

## Data Safety

- Declare every table once in its feature schema as `user`, `cache` or `marker`.
  Registry order follows creation dependencies; cache clearing walks it backwards.
  Use `external` for tables created outside the macro, such as FTS5.
- User facts include names, manual geometry, reviews/rejections, references,
  history, roots, tags and file recovery. Cache clear, format reset, indexing and
  analysis must never delete them. Only an explicit confirmed user action may
  delete user facts. Never delete by table-name prefix or drop a database.
- Keep cache/user writes inside their owning feature operations. Named workflow
  calls express cross-feature writes; source audits supplement behavior tests.
- Current application state is isolated under `core-v2`, including custom
  preview-cache roots. Current SQLite schema is version 2. Unknown, old or future
  populated files are rejected read-only before writer/WAL setup; do not silently
  adopt, migrate, reset or delete them. Future upgrades require an explicit
  user-preserving design. Verify with fresh isolated state.
- Product preview-clear removes current preview files and invalidates requests;
  it does not grant a capability to clear SQLite user tables.

## Performance and Lifetime

- Folder open stays cheap, non-recursive and paged. Defer indexing, metadata,
  preview generation, decode and model loading from the interactive open path.
- Never send image bytes through JSON IPC. Use paths, protocols, committed
  projections and existing cache boundaries.
- Keep blocking and CPU-heavy work off async/UI threads. Core owns application
  admission and queues; media owns native execution. Preserve priority,
  cancellation, generation, selection order, invalidation, leases and stale-result
  rejection. Do no filesystem/database work under queue locks.
- Worker lifetime belongs to CoreRuntime or the explicitly joined snapshot
  owner. Stop rejects admission/publication, cancels cooperative work, releases
  waiters and wakes gates. A native call that exceeds the shutdown deadline is
  reported as a timeout, never as joined. Slow WebView consumers do not block
  workers holding locks.
- Index scheduling belongs to library, directory-tree scheduling to browse,
  preview/metadata projections and queue diagnostics to resources. Preserve
  separate budgets and independent WAL read paths.

## Change Discipline

- Preserve unrelated edits; stage and commit coherent groups explicitly.
- Add focused tests near changed behavior. Do not hand-edit target, dist,
  tsbuildinfo, generated Tauri schemas or icons. Contract output is regenerated
  through its pinned exporter rather than manually patched.
- Update the current document selected by docs/README when changing architecture,
  formats, performance, native inputs or roadmap status. Keep dated measurements
  under docs/research and completed/superseded plans under docs/archive.
- Run applicable CONTRIBUTING checks. Cross-boundary changes need frontend and
  Rust verification. Media/browsing performance needs a release build and a
  representative real fixture compared with docs/PERFORMANCE; build success
  alone does not prove visible behavior or performance.
- Treat native dependencies as pinned inputs. Do not change submodules, vcpkg,
  native source manifests, or notices unless the task requires it.
- Close application instances started for verification when finished.
