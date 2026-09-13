# AGENTS.md

Repository-specific rules for coding agents working on OxyViewer.

## Start Here

- Use [CONTRIBUTING.md](CONTRIBUTING.md) for setup, build commands, and the
  verification matrix.
- Use [docs/README.md](docs/README.md) to find the authoritative architecture,
  format, packaging, roadmap, task, and research documents.
- Rust changes must follow [docs/RUST_STYLE.md](docs/RUST_STYLE.md). New
  workspace crates must use `[lints] workspace = true`; do not add crate-wide
  Clippy exemptions.
- Before changing browsing, media caches, request queues, scheduling, or queue
  diagnostics, read
  [docs/PERFORMANCE_INVARIANTS.md](docs/PERFORMANCE_INVARIANTS.md).

## Non-Negotiable Contracts

- Opening a folder must remain cheap, non-recursive, and paged. Defer indexing,
  metadata, preview generation, and decode work from the interactive open path.
- Keep `apps/desktop/src-tauri` thin. Put reusable behavior in the appropriate
  Rust crate.
- Put public serialized Rust contracts in `oxy-domain` with camelCase serde
  names. Keep frontend IPC wrappers in `apps/desktop/src/lib/api.ts`, updating
  TypeScript types and browser-demo behavior with command changes.
- Never send image bytes through JSON IPC. Use paths, protocols, projections,
  or the existing cache boundaries.
- Keep blocking and CPU-heavy media work off async and UI threads. Preserve
  priority, cancellation, generation, invalidation, lease, and stale-result
  semantics when replacing an implementation.
- Treat the SQLite library and generated previews as rebuildable caches.
- Route file discovery and safe file operations through `oxy-fs`; do not add
  ad hoc frontend filesystem access.
- Treat native dependencies as pinned inputs. Do not change submodules, vcpkg
  overlays, FFmpeg sources, or third-party notices unless the task requires it.

## Change Discipline

- Keep changes scoped and preserve unrelated worktree edits.
- Add focused tests near changed behavior. Do not hand-edit generated outputs
  such as `target`, `apps/desktop/dist`, `*.tsbuildinfo`, generated Tauri
  schemas, or generated icons.
- Update the current document selected through `docs/README.md` when changing
  architecture, format support, performance behavior, native dependencies, or
  roadmap status. Keep dated measurements under `docs/research` and completed
  plans under `docs/archive`.
- Run the applicable checks from `CONTRIBUTING.md`. Cross-boundary changes need
  both frontend and Rust verification.
- For performance-sensitive media or browsing work, use a release build and a
  representative real fixture, then compare against
  [docs/PERFORMANCE.md](docs/PERFORMANCE.md). Build success alone is not a
  performance or visible-behavior verification.
- Close any debug application instance started for verification when finished.
