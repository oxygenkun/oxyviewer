# oxy-media `lib.rs` simplification

Status: Completed; public API details below were superseded by the final convergence recorded in
`docs/architecture/07-media-refactoring.md`.

## Goal

Reduce `crates/oxy-media/src/lib.rs` to a small crate facade while preserving the
existing preview behavior, cache identities, fallback ordering, diagnostics,
platform-specific behavior, and performance characteristics.

This is an equivalent refactor. It must not redesign the HEIF tile session or
force RAW, HEIF, and system previews behind an artificial common decoder trait.

## Constraints

- Keep `src-tauri` thin; reusable media behavior remains in `oxy-media`.
- Preserve `oxy_media::preview(...)` as the desktop preview entry point.
- Do not change cache version strings during file/module movement.
- Preserve decode priorities, locks, fallback ordering, artifact suffixes,
  representation contracts, and `PreviewDiagnostics` behavior.
- Keep format quirks in `formats/`, decoder adapters in `backends/`, and request
  planning/execution in `pipeline/`.
- Do not remove code required by a supported platform merely because it is not
  reachable on the current development platform.
- Follow `docs/RUST_STYLE.md`; do not introduce broad lint exemptions.

## Steps

### 1. Baseline and API inventory

- [x] Record the public API used outside `oxy-media` and by package binaries.
- [x] Identify compatibility-only APIs and stale compatibility comments.
- [x] Confirm the existing focused tests cover dispatcher fallback, cache reuse,
      RAW representation substitution, and HEIF diagnostics.

Inventory result:

- Tauri uses `preview` (passing domain `PreviewPriority` directly), `dimensions`,
  cache maintenance, `preview_policy_revision`, `HeifDecodeService`, and `MediaError`.
- `start_heif_full` is now the single Tauri full-delivery boundary; the earlier standalone cache
  lookup command was removed.
- HEIF preview production now has one internal `heif::artifact::preview`
  function. `heif_display_bench` uses the public semantic dispatcher instead
  of preserving a separate exact-size facade.
- The RAW benchmark now uses the unified dispatcher. RAW/system/full helpers
  are crate-private.
- The unused public `HeifBackend`/`TileSink` shim and the unreachable in-process
  high-bit-depth HEIF conversion path were removed. Portable libheif RGB8
  preview/session fallbacks remain active.
- Existing tests cover planner fallback pairs, cancellation, cache reuse, RAW
  representation substitution, HEIF timing diagnostics, fixtures, and ignored
  performance budgets.

### 2. Extract shared types and source inspection

- [x] Move `MediaError` and `ImageDimensions` out of `lib.rs` into focused
      modules, re-exporting them so existing callers remain source-compatible.
- [x] Move generic `dimensions`/result construction helpers to a focused module.
- [x] Keep RAW-specific dimension probing with the RAW pipeline.

### 3. Move format execution out of `lib.rs`

- [x] Move RAW preview/full/cache execution into `pipeline::raw`.
- [x] Move HEIF preview/full/source-JPEG execution into
      `pipeline::heif::artifact` and backend strategy into
      `pipeline::heif::backend`, without mixing either into decoder adapters.
- [x] Move system preview execution into its own pipeline module.
- [x] Keep platform `cfg` branches close to the execution code they control.

### 4. Isolate unified dispatch

- [x] Move `preview`, decode-plan execution, and platform selection into
      `pipeline::dispatcher`; keep domain-to-gate priority conversion private in
      `decode_control`.
- [x] Preserve the two explicitly supported fallback pairs and cancellation
      behavior.
- [x] Re-export only the dispatcher and stable facade APIs from `lib.rs`; gate
      priority types remain private implementation details.

### 5. Reduce shims and duplicated cache flow

- [x] Convert format-specific preview functions to crate-private unless an
      external command or benchmark still needs them.
- [x] Update benchmark binaries to exercise the unified dispatcher where this
      does not change what is measured.
- [x] Consolidate exact/up-tier artifact lookup and atomic commit helpers only
      where representation semantics remain explicit.
- [x] Audit `HeifBackend`, the cached-full HEIF lookup, and the `libheif` dead-code
      exemption; remove or narrow only code proven unused on all supported
      platforms.
- [x] Correct stale compatibility comments and names without changing IPC
      command names unnecessarily.

The shared artifact module owns progressive cache sizes and timing conversion;
atomic writes remain in `cache`. RAW and HEIF keep separate up-tier lookup
because their artifact suffixes and representation semantics differ.

### 6. Tests and documentation

- [x] Move tests out of the facade and keep focused module tests beside their
      implementation; cross-pipeline fixture/performance tests live in
      `src/tests.rs`.
- [x] Keep fixture-dependent and performance tests available as ignored tests.
- [x] Update preview-pipeline documentation if module ownership or compatibility
      descriptions changed.

### 7. Verification

- [x] `cargo fmt --all --check`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `cargo test --workspace`
- [x] Review the final diff for cache-version, fallback, and platform-`cfg`
      regressions.

Verification completed on macOS. Windows/Linux `cfg` branches compile in CI;
this refactor keeps their existing backend calls but they were not cross-built
locally.

## Completion criteria

- `lib.rs` is primarily module declarations and public re-exports.
- The Tauri preview worker still calls one unified `oxy_media::preview` entry.
- Supported benchmark binaries still build and measure the intended paths.
- No broad `dead_code` allowance remains solely to hide obsolete compatibility
  code unless its supported-platform need is documented.
- All required Rust checks pass.
