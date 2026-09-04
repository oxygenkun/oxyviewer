# Contributing to OxyViewer

Thank you for helping improve OxyViewer. This guide covers the development
workflow and the architectural constraints that contributions must preserve.

## Before You Start

For a substantial change, open an issue or discussion first so its scope and
approach can be agreed before implementation. Small fixes, focused tests, and
documentation improvements can usually go directly to a pull request.

Please keep each contribution focused. Do not include generated build output or
unrelated formatting changes.

## Development Setup

You will need:

- Node.js 22
- pnpm 10.34.5 (the version pinned in `package.json`)
- Rust 1.85 or newer with `rustfmt` and `clippy` (the workspace uses edition 2024)
- The platform prerequisites required by Tauri 2

Clone the repository with its submodules, then install the frontend
dependencies:

```bash
git clone --recurse-submodules <repository-url>
cd oxyviewer
pnpm install --frozen-lockfile
```

If you already cloned the repository without submodules, initialize them with:

```bash
git submodule update --init --recursive
```

The vendored LibRaw source under `3rdpart/libraw` is a submodule. Do not modify
or update vendored dependencies unless the contribution specifically requires
it.

## Running OxyViewer

Run the browser-only frontend during UI development:

```bash
pnpm dev
```

Outside Tauri, the frontend supplies a small demonstration folder. To run the
desktop application and exercise native commands:

```bash
pnpm tauri dev
```

Run all commands in this guide from the repository root. Use `pnpm icons` only
when intentionally regenerating application icons.

## Project Structure

- `apps/desktop/src`: React frontend and browser demo behavior
- `apps/desktop/src-tauri`: thin Tauri command, state, and protocol layer
- `crates/oxy-domain`: serialized contracts shared across Rust boundaries
- `crates/oxy-fs`: discovery, path identity, sidecars, and file operations
- `crates/oxy-media`: previews, thumbnails, and native media adapters
- `crates/oxy-metadata-parser`: in-process EXIF/XMP/IPTC/ICC/MakerNote parser
- `crates/oxy-metadata`: normalized metadata, sidecars, and optional ExifTool boundary
- `crates/oxy-library`: rebuildable SQLite cache and library roots
- `crates/oxy-runtime`: job priority and cancellation vocabulary
- `docs`: architecture, decisions, roadmap, formats, and performance budgets

Read `docs/ARCHITECTURE.md` before making a cross-layer change. The focused
guides under `docs/architecture` describe the main runtime and extension paths.

## Architecture and Performance Rules

Opening a folder must remain immediate. Return cheap, non-recursive, paged file
summaries first, and defer indexing, metadata reads, preview generation, and
decoding.

When changing the application:

- Keep `src-tauri` thin and put reusable behavior in the appropriate Rust crate.
- Put public serialized Rust contracts in `oxy-domain`, using camelCase serde
  naming to match TypeScript.
- Keep frontend IPC wrappers in `apps/desktop/src/lib/api.ts`. Update their
  TypeScript types and browser demo behavior when applicable.
- Never send image bytes through JSON IPC. Return paths or use the existing
  protocol and cache boundaries.
- Move blocking or CPU-heavy work off the async and UI threads.
- Preserve job priority, cancellation, and stale-result handling semantics.
- Treat generated previews and the SQLite library as rebuildable caches.
- Route safe file operations through `oxy-fs`; do not add ad hoc frontend
  filesystem access.

For browsing or media-pipeline changes, review `docs/PERFORMANCE.md` and verify
that first-page rendering, virtualized scrolling, and preview latency do not
regress.

## Tests and Verification

Add focused tests near the changed logic. Rust unit tests generally live beside
their implementation. Frontend tests use Vitest and are named `*.test.ts` or
`*.test.tsx`.

For frontend-only changes, run:

```bash
pnpm check
pnpm test
pnpm build
```

For Rust-only changes, run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For Tauri commands, shared contracts, IPC, or other cross-boundary changes, run
both groups. Media performance changes should also run the relevant release
benchmark and compare the result with `docs/PERFORMANCE.md`.

## Documentation

Update documentation as part of the same contribution when behavior or
architecture changes:

- Architecture or IPC boundaries: `docs/ARCHITECTURE.md` or a focused guide
- Format/backend support: `docs/FORMAT_SUPPORT.md`
- Browsing, scheduling, caching, color, or cancellation behavior:
  `docs/PERFORMANCE.md`
- A durable decision with meaningful tradeoffs: add an ADR under `docs/adr`
- Implementation status: `docs/ROADMAP.md`

ADRs should describe the context, decision, consequences, negative tradeoffs,
and alternatives considered.

## Pull Requests

In the pull request description:

- Explain the problem and the chosen solution.
- Call out user-visible, architectural, compatibility, or performance effects.
- List the checks you ran and any checks you could not run.
- Include screenshots or a short recording for meaningful UI changes.
- Link the related issue or discussion when one exists.

Before requesting review, inspect the diff for accidental generated files,
secrets, local paths, and unrelated changes. CI runs formatting, Clippy, Rust
tests, frontend checks and builds, followed by desktop builds on macOS, Windows,
and Linux.
