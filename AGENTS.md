# AGENTS.md

This file gives coding agents repository-specific guidance for OxyViewer.

## Project Overview

OxyViewer is a local-first photo browser built with Rust, Tauri 2, React 19,
and TypeScript. Opening a folder must remain immediate: return cheap,
non-recursive, paged file summaries first and defer expensive preview,
metadata, and indexing work.

## Repository Layout

- `apps/desktop/src`: React frontend and browser-only demo behavior.
- `apps/desktop/src-tauri`: thin Tauri command layer and application state.
- `crates/oxy-domain`: serialized contracts shared across Rust boundaries.
- `crates/oxy-fs`: discovery, path identity, sidecars, and file operations.
- `crates/oxy-media`: preview, thumbnail, and native media adapters.
- `crates/oxy-metadata-parser`: in-process EXIF/XMP/IPTC/ICC/MakerNote parser.
- `crates/oxy-metadata`: normalized metadata, sidecars, and optional ExifTool boundary.
- `crates/oxy-library`: rebuildable SQLite cache and library roots.
- `crates/oxy-runtime`: job priority and cancellation vocabulary.
- `docs`: architecture, roadmap, format support, performance budgets, and ADRs.
- `3rdpart/libraw`: vendored LibRaw source (git submodule); do not modify casually.

## Development Commands

Run commands from the repository root.

```bash
pnpm install
pnpm dev                 # browser-only frontend development
pnpm tauri dev           # desktop application
pnpm check               # TypeScript checks
pnpm test                # frontend tests
pnpm build               # frontend production build
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Use `pnpm icons` only when intentionally regenerating application icons.

Rust changes must also follow [`docs/RUST_STYLE.md`](docs/RUST_STYLE.md). New
workspace packages must opt into the shared lint policy with
`[lints] workspace = true`; do not add crate-wide Clippy exemptions.

## Architecture Rules

- Keep `src-tauri` thin. Put reusable behavior in the appropriate Rust crate.
- Put public serialized Rust contracts in `oxy-domain` and use camelCase serde
  naming to match TypeScript.
- Keep frontend IPC wrappers in `apps/desktop/src/lib/api.ts`. When adding or
  changing a Tauri command, update its TypeScript types and browser demo
  behavior when applicable.
- Never send image bytes through JSON IPC. Return paths or use an appropriate
  protocol/cache boundary.
- Do not recursively scan or synchronously index a folder during open.
- Move decode, preview generation, metadata work, and other blocking operations
  off the async/UI thread. Preserve job priority and cancellation semantics.
- Treat the SQLite library and generated previews as rebuildable caches.
- Preserve safe file-operation contracts; do not bypass `oxy-fs` with ad hoc
  frontend filesystem behavior.

## Editing Guidance

- Follow existing code style and keep changes scoped to the requested behavior.
- Add focused tests near changed logic. Rust unit tests generally live beside
  the implementation; frontend tests use Vitest and `*.test.ts`/`*.test.tsx`.
- Avoid hand-editing generated outputs such as `target`, `apps/desktop/dist`,
  `*.tsbuildinfo`, generated Tauri schemas, and generated icons.
- Do not update vendored third-party dependencies or third-party notices unless the
  task explicitly requires it.
- Update relevant files in `docs` when changing architecture, format support,
  performance behavior, native dependencies, or roadmap status.

## Verification

Choose checks according to the change, then run the broad suite for shared or
cross-boundary work.

- Frontend-only: `pnpm check && pnpm test && pnpm build`
- Rust-only: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- Cross-boundary/Tauri: run both frontend and Rust checks.

For performance-sensitive browsing or media changes, verify against the budgets
in `docs/PERFORMANCE.md` and avoid regressions to first-page rendering,
virtualized scrolling, or preview latency.

Before refactoring browsing, media caches, request queues, or queue diagnostics,
read [`docs/PERFORMANCE_INVARIANTS.md`](docs/PERFORMANCE_INVARIANTS.md). Preserve its
concurrency, invalidation, lease, whole-folder thumbnail retention, and nonblocking
diagnostic contracts; validate equivalent behavior if replacing the implementation.

## Behavior

Close debug instance when finished
