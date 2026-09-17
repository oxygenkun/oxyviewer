# Repository Scripts

This directory is grouped by **how a script is executed**, because that decides
its extension, its working directory, and whether it can run in CI at all.
Commands and the verification workflow live in
[CONTRIBUTING.md](../CONTRIBUTING.md); this file records what each group is and
the constraints the layout depends on.

The performance harness is **not** here: its runner, configuration, fixtures,
and reports are one unit under [`tests/perf/`](../docs/PERF_E2E.md), so a
scenario and the code that runs it stay together.

## Groups

| Directory | Executed by | Entry point |
| --- | --- | --- |
| `release/` | `node` and PowerShell, from CI | `.github/workflows/ci.yml` |
| `browser/` | Pasted into a browser or WebView page | not runnable by Node |

### `release/`

- `check-release-version.mjs` — asserts that the root, desktop, Tauri, and Cargo
  versions agree. `pnpm release:check`; CI calls it on tags.
- `extract-release-notes.mjs` — prints the CHANGELOG section for a tag; CI
  redirects it to `release-notes.md`.
- `package-windows-portable.ps1` — builds the Windows portable archive.

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

The `.browser.js` suffix is deliberate: it keeps these probes distinguishable
from Node entry points and from the `.mjs` scripts in `release/`.

These probes locate modules by `/src/...` path, so a frontend file move breaks
them silently. `refactor(desktop): group frontend sources by domain` (ed19026)
broke every probe path and none were updated; verify the `/src/...` targets
exist after moving frontend files. Prefer extending the in-app
diagnostics probes under `apps/desktop/src/lib/diagnostics/`, which are
covered by type checks and tests and are driven by `perf-e2e.mjs`
scenarios, when a measurement can live there instead.

## Constraints

- **Node scripts here must be `.mjs`.** The root `package.json` has no
  `"type": "module"`, so a plain `.js` file in this directory is parsed as
  CommonJS and `import` fails. `.mjs` is always ESM, regardless of the nearest
  package scope. The same rule applies to `tests/perf/*.mjs`.
- **Run `release/` scripts from the repository root.** They resolve
  `package.json`, `CHANGELOG.md`, and `target/` relative to the current
  directory.
- **`package-windows-portable.ps1` derives the repository root from
  `$PSScriptRoot`.** Moving it again requires updating that expression.

## Related tooling

- `tests/perf/perf-e2e.mjs` — the E2E performance runner; see
  [PERF_E2E.md](../docs/PERF_E2E.md). It derives the repository root from
  `import.meta.url`, so it runs from any directory.
- `apps/desktop/scripts/tauri.mjs` — prepares pinned native dependencies before
  `tauri` runs; see [FFMPEG_PACKAGING.md](../docs/FFMPEG_PACKAGING.md).
- `apps/desktop/scripts/generate-icons.mjs` — generates the desktop icon source
  PNG; `pnpm icons` runs it before `tauri icon`.
- `3rdpart/*/prepare.mjs` — native source preparation and bundle verification.
