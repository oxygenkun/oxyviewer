# Repository Scripts

This directory is grouped by **how a script is executed**, because that decides
its extension, its working directory, and whether it can run in CI at all.
Commands and the verification workflow live in
[CONTRIBUTING.md](../CONTRIBUTING.md); this file records what each group is and
the constraints the layout depends on.

## Groups

| Directory | Executed by | Entry point |
| --- | --- | --- |
| `release/` | `node` and PowerShell, from CI | `.github/workflows/ci.yml` |
| `perf/` | `node` and PowerShell, manually from the repo root | `pnpm perf:e2e` |
| `browser/` | Pasted into a browser or WebView page | not runnable by Node |

### `release/`

- `check-release-version.mjs` — asserts that the root, desktop, Tauri, and Cargo
  versions agree. `pnpm release:check`; CI calls it on tags.
- `extract-release-notes.mjs` — prints the CHANGELOG section for a tag; CI
  redirects it to `release-notes.md`.
- `package-windows-portable.ps1` — builds the Windows portable archive.

### `perf/`

- `perf-e2e.mjs` — the E2E performance runner. Launches the packaged release app
  with `OXY_PERF_SCENARIO`, reads `tests/perf/scenarios.json` and
  `tests/perf/baseline.json`, and writes reports under `tests/perf/.reports/`.
  See [PERF_E2E.md](../docs/PERF_E2E.md).
- `perf-scroll.mjs` — standalone CDP wheel probe attached to an explicitly
  supplied WebView debugging port.
- `raw-backend-bench.ps1` — Windows RAW backend benchmark; writes to
  `tests/perf/.reports/raw-backend`.

### `browser/`

Manual regression probes. These are **not** Node scripts: they fetch
`/src/*.tsx` from a Vite dev server and stub `window.__TAURI_INTERNALS__`, so
they only run inside a page served by `pnpm dev`. Pass one to
`agent-browser --session <name> eval` using the command in the file header.

| Probe | Covers |
| --- | --- |
| `browse-startup.browser.js` | App/IPC startup states with controlled native responses |
| `filmstrip-pagination.browser.js` | Virtualized Loupe filmstrip paging |
| `loupe-switch.browser.js` | Photo switching and stale image layers |
| `preview-geometry.browser.js` | Preview geometry with late metadata |
| `folder-thumbnail-retention.browser.js` | Folder thumbnail retention; needs its host page |
| `grid-frame-time.browser.js` | Grid scroll frame timing |
| `folder-thumbnail-probe.html` | Host page for the retention probe, opened through Vite's `/@fs/` route |

The `.browser.js` suffix is deliberate: it keeps these probes distinguishable
from Node entry points and from the `.mjs` scripts in `release/` and `perf/`.

## Constraints

- **Node scripts here must be `.mjs`.** The root `package.json` has no
  `"type": "module"`, so a plain `.js` file in this directory is parsed as
  CommonJS and `import` fails. `.mjs` is always ESM, regardless of the nearest
  package scope.
- **Run `release/` and `perf/` scripts from the repository root.** They resolve
  `package.json`, `CHANGELOG.md`, and `tests/perf/` relative to the current
  directory. `perf-e2e.mjs` is the exception: it derives the root from
  `import.meta.url`.
- **`package-windows-portable.ps1` derives the repository root from
  `$PSScriptRoot`.** Moving it again requires updating that expression.

## Related tooling

- `apps/desktop/scripts/tauri.mjs` — prepares pinned native dependencies before
  `tauri` runs; see [FFMPEG_PACKAGING.md](../docs/FFMPEG_PACKAGING.md).
- `apps/desktop/scripts/generate-icons.mjs` — generates the desktop icon source
  PNG; `pnpm icons` runs it before `tauri icon`.
- `3rdpart/*/prepare.mjs` — native source preparation and bundle verification.
