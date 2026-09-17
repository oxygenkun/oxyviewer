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
- CMake and a C/C++ compiler for the linked native media libraries
- `pkg-config` on macOS and Linux (`brew install pkg-config` or the
  distribution package); `libheif-sys` locates the pinned libheif through it
- NASM on `PATH`, or the `NASM` environment variable pointing to it, for the
  libjpeg-turbo SIMD backend on x86/x64

On Windows, install the pinned libheif before building the Rust workspace
(replace `C:\vcpkg` if `VCPKG_ROOT` points elsewhere):

```powershell
C:\vcpkg\vcpkg.exe install "libheif[core]:x64-windows-static-md"
```

libheif resolves from the vcpkg curated registry, so the checkout must be at or
after vcpkg commit `0635f447edcc25f50645afade4e91a229a35fcdc`, the revision that
added the 1.23.4 port that satisfies the security baseline. Run
`git -C C:\vcpkg pull` first if an older version is selected. CI checks out that
revision explicitly instead of using the one bundled with the runner image. The
Windows libheif decodes HEVC through libde265; it is not linked against FFmpeg.

Clone the repository with its submodules, then install the frontend
dependencies:

```bash
git clone --recurse-submodules <repository-url>
cd oxyviewer
pnpm install --frozen-lockfile
```

On macOS and Linux, build the pinned native media libraries before any Cargo
command. Cargo resolves the resulting static libheif through
`.cargo/config.toml`; without the prefix, `cargo build` and `cargo test` fail to
find libheif:

```bash
pnpm native:prepare
```

`native:prepare` builds the pinned FFmpeg (programs plus a static prefix) and
then the pinned libheif with `WITH_FFMPEG_DECODER=ON`. It is cached under
`target/` and safe to re-run. Windows skips the libheif step and uses vcpkg.

If you already cloned the repository without submodules, initialize them with:

```bash
git submodule update --init --recursive
```

The LibRaw and libjpeg-turbo sources under `3rdpart` are pinned submodules.
Cargo compiles both automatically: LibRaw through `cc`, and libjpeg-turbo
through its upstream CMake `jpeg-static` target. Do not modify or update pinned
native dependencies unless the contribution specifically requires it.

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

Build the browser frontend without packaging the desktop application:

```bash
pnpm build
```

Build release desktop packages through the repository wrapper:

```bash
pnpm tauri build
```

Release builds also download, build, cache, verify, and stage the pinned
standalone FFmpeg/ffprobe programs. Run `pnpm native:prepare` to perform the
native media steps independently (`pnpm ffmpeg:prepare` covers only FFmpeg). Do
not bypass the wrapper with `pnpm exec tauri` for a
release build; see `docs/FFMPEG_PACKAGING.md` for platform prerequisites,
bundle verification, signing, and redistribution requirements.

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
- `scripts`: repository tooling, grouped into `release/` and browser
  regression probes; see [scripts/README.md](scripts/README.md)
- `tests/perf`: the E2E performance runner, its scenarios and baseline, and the
  ignored generated fixtures and reports; see [docs/PERF_E2E.md](docs/PERF_E2E.md)

Read `docs/ARCHITECTURE.md` before making a cross-layer change. The focused
guides under `docs/architecture` describe the main runtime and extension paths.

Use the [documentation index](docs/README.md) to choose the current contract,
focused architecture guide, active plan, or historical evidence relevant to a
substantial change.

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

For Rust-only changes, run (after `pnpm native:prepare` on macOS and Linux):

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
secrets, local paths, and unrelated changes.

## Releases, CI, and build costs

The release version is declared in the root and desktop `package.json` files,
the Cargo workspace package metadata, and `tauri.conf.json`. Keep all four
values identical. Run `pnpm release:check` before tagging; CI also checks that
the tag is exactly `v<version>`.

Push a semantic version tag such as `v0.1.0` to run the complete release
pipeline. After all checks and platform builds succeed, CI creates or updates
the matching GitHub Release and attaches the macOS DMG, multilingual Windows
NSIS EXE, Windows portable ZIP, Linux DEB, Linux AppImage, and the corresponding
FFmpeg source archive.
The NSIS installer follows the operating-system language for English and
Simplified Chinese. Published filenames include the target platform; the Windows
installer is named `OxyViewer_<version>_Windows_x64.exe` without the bundler's
`-setup` suffix. Re-running a completed release replaces assets with the newly
verified packages, removes obsolete installer names, and refreshes the release
description from the matching version section in `CHANGELOG.md`. A missing or
empty section fails the release instead of publishing incomplete notes.

The `build` tag no longer triggers CI. For a Windows test package, use
**Actions → CI → Run workflow** and select the ref to build. The `platform`
input defaults to `windows` and also accepts `macos`, `linux`, or `all`; the
`all` and `linux` selections additionally run the Clippy and workspace test job.
Manual runs skip the Linux formatting/frontend checks, so these packages have
not necessarily passed the release checks. The workflow must first exist on the
default branch to expose manual dispatch.

Pushing a `v<major>.<minor>.<patch>` tag runs formatting and frontend checks on
Linux, followed by Clippy and workspace Rust tests. Only after both jobs succeed
do Windows, macOS, and Linux package builds start. Linux packaging reuses the
workspace test result; Windows and macOS run their native media and desktop
tests. All packages undergo FFmpeg payload verification. To retry an unchanged
failed build, use **Re-run failed jobs** instead of re-running successful
platforms.

New manual runs cancel superseded manual runs of the same ref, independently
of release runs. Checks and builds
have explicit timeouts. Manual build artifacts expire after 7 days; release-tag
Actions artifacts expire after 30 days. Published GitHub Release installers
remain attached to the release.

Windows vcpkg and Rust link caches include the MSVC toolset version (falling
back to the runner image version when the toolset is not exported), the pinned
vcpkg revision, and the core-only feature selection in their keys. The toolset
version changes only when the compiler does, so an unrelated runner image update
still reuses the cached binary packages instead of rebuilding them.

The pinned FFmpeg and libheif builds are deliberately not cached in CI. Every
job that links them (the Linux checks and all three package builds) prepares and
verifies them from the pinned sources through `prepare.mjs` before Tauri
packaging, so a release always compiles the native libraries it ships. That costs
a few minutes per platform, and `prepare.mjs` still reuses its own tree when the
source/recipe/target and binary checksums match, which only shortens repeated
local builds. `target/ffmpeg`, `target/libheif` and `target/native` are local
build directories, not CI artifacts.

Rust dependency and Windows vcpkg caches stay best-effort. Only the default
branch writes them, so a cache saved by one version tag is invisible to every
other tag and a cache saved by a branch is invisible to every release; tag and
branch runs restore them without saving. Because `ci.yml` runs only for version
tags and manual dispatch, run it on `main` with `platform=all` before tagging
after a dependency change, a change to a cache key or the pinned vcpkg revision,
or a quiet period longer than a week; any of these can leave those caches stale,
and failed default-branch jobs still save through `cache-on-failure`.

Private-repository Actions usage can incur charges after the account allowance
is exhausted. Job timeouts limit individual runs, not monthly spending. Check
the account's **Billing & licensing** usage and budgets for the actual net
charges. A payment or budget block must be resolved in account settings; a
workflow change cannot clear it.

## Contribution Licensing

Except for contributions to components that explicitly state different
license terms, by intentionally submitting a contribution for inclusion in
OxyViewer, you represent that you have the right to license it and agree that:

- the contribution may be distributed to the public as part of OxyViewer under
  AGPL-3.0-only; and
- you grant the OxyViewer repository owner and copyright holder a perpetual,
  worldwide, non-exclusive, royalty-free, irrevocable right to use, reproduce,
  modify, distribute, sublicense, and relicense the contribution as part of
  OxyViewer, including under OxyViewer commercial licenses.

If you cannot grant these rights, do not submit the contribution without first
making a separate written arrangement with the repository owner at
[oxygenkun.1@gmail.com](mailto:oxygenkun.1@gmail.com).
