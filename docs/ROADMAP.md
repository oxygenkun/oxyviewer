# OxyViewer Roadmap

Status markers: `[x]` complete, `[~]` active, `[ ]` planned.

This roadmap describes the current `main` branch rather than the order in which
features originally landed. Last reviewed: 2026-09-13.

## Current Status

OxyViewer already has a functional local-first browser, persistent library,
progressive RAW/HEIF display, native metadata engine, sidecar editing, and
rebuildable caches. The project is still pre-release because its large-directory
performance gate, cross-platform media matrix, file-operation recovery, and
signed distribution work are incomplete.

The main release blockers are:

- the measured 100k-file first page takes about 2.3 s instead of the 300 ms target;
- RAW/HEIF fixtures and packaged codecs are not validated across all release targets;
- external filesystem changes still require manual refresh;
- file writes lack a session-scoped authorization contract, undo journal, and recovery;
- accessibility, crash recovery, signing, notarization, and installer validation remain.

## Phase 1: Core Browser and Local Library

This phase combines the former engineering-foundation, folder-browsing, and
library phases. The remaining work is now dominated by scale and consistency,
not by basic UI construction.

### Implemented

- [x] Cargo/pnpm workspaces, Rust 2024 lint policy, Tauri 2 shell, logging, i18n,
  shared domain contracts, and macOS/Windows/Linux CI builds
- [x] Native folder picker and cheap, non-recursive folder sessions
- [x] Paged asset queries with search, type/rating/color filters, sorting, and
  virtualized grid/list rendering
- [x] Grid, list, progressive loupe, filmstrip, multi-selection, and keyboard navigation
- [x] Rust-owned, revisioned directory trees with on-demand child loading and
  active-directory priority scheduling
- [x] Persistent, reorderable library roots and background SQLite WAL indexing
- [x] Asset FTS search plus ancestor-preserving directory search inside an indexed root
- [x] Configurable, size-bounded preview cache with safe clear/prune behavior
- [x] Rename/copy/move/trash service contracts with sidecar pairing, collision
  rejection, and recoverable system trash

### Remaining

- [~] Replace the synchronous full-directory snapshot on the first uncached page.
  The 2026-09-02 release E2E run measured about 18 ms for `open_folder`, 2.2 s
  for the first 250-item page, and 2.3 s to first-page paint for 100k files.
- [ ] Add filesystem watching and streamed `folder_delta` updates without making
  folder open recursive or blocking.
- [ ] Add cross-root asset search and metadata-aware indexed filters. Current FTS
  queries cover one loaded root; rating/color filters still enrich current-directory
  metadata progressively.
- [ ] Put file writes behind an explicit folder-session/root authorization policy,
  then add complete dialogs, partial-failure reporting, an undo journal, and recovery.

**Gate:** on a local SSD release build, a 100k-file directory begins rendering
within 300 ms, remains virtualized while scrolling, and reflects external changes
without a blocking rescan.

## Phase 2: Media and Metadata Reliability

This phase combines the former image-pipeline and metadata phases. Semantic render
levels, scheduling, native reads, and sidecar writes are implemented; the focus is
now fixture coverage, cancellation, platform packaging, and conflict safety.

### Implemented foundation

- [x] Semantic `thumbnail → preview → full` render graph with Rust-owned image
  projections, stale-result rejection, and SQLite restart persistence
- [x] Scoped `loupe / visible / nearby / preload` scheduling with request
  coalescing, two preview workers, up-tier cache reuse, and a shared decode gate
- [x] Direct JPEG/PNG/WebP display and semantic TIFF preview routing
- [x] Pinned LibRaw 0.22.2 source submodule build, dimensions, embedded RAW preview, half-size
  fallback, 512/4096 caches, and full-resolution development
- [x] Near-full-size RAW embedded-JPEG fast path and measured macOS ImageIO/Core Image RAW JPEG fallbacks
- [x] HEIF embedded-preview fast path, ICC/HDR-to-SDR conversion, and bounded
  source decoding
- [x] Direct ImageIO full JPEG on macOS; progressive RGBA/JPEG tile sessions on
  Windows/Linux with a source-derived full JPEG warm cache
- [x] Native `oxy-metadata-parser` engine for EXIF/XMP/IPTC/ICC/MakerNotes, with
  `libheif-rs` item-table XMP extraction for HEIF/HIF
- [x] Sony ARW/JPEG/HEIF shooting-focus overlay with persistent and temporary controls
- [x] Sidecar-first rating/color/flag reads and multi-selection writes for all
  supported formats, including embedded RAW XMP preservation on first edit
- [x] Optional ExifTool configuration and checksum-verified managed install for
  explicit JPEG/HEIF/HIF embedded synchronization; it is not a core bundled worker

### RAW reliability

- [ ] Validate ARW, CR2, CR3, NEF, DNG, RAF, RW2, and ORF with distributable
  orientation, color, embedded-preview, damaged-file, and full-detail fixtures.
- [ ] Validate the pinned LibRaw build and RAW preview/full behavior on Windows
  and Linux release artifacts, not only compile-only or synthetic coverage.
- [ ] Add cooperative cancellation checkpoints where possible. Pending work can
  be reordered and consumers can stop waiting, but an active native decode is not
  generally preemptible.

#### Optional macOS full-size backend evaluation

- [ ] Benchmark Core Image `CIRAWFilter` against LibRaw for cold latency, peak
  memory, dimensions, orientation, color, and 100% detail before selecting it.
- [ ] If it wins, add a bounded macOS adapter with a distinct cache/backend version,
  diagnostics, and automatic LibRaw fallback while retaining the visible preview.
- [ ] Keep the embedded-JPEG fast path first and keep Windows/Linux fully portable;
  Core Image must never become a prerequisite for thumbnail or preview display.

### HEIF reliability

- [ ] Follow the [HIF foreground/background acceleration plan](tasks/hif-performance-plan.md):
  display-state cache correctness and background DCT stitching are implemented;
  concurrent performance matrices and measured foreground worker reuse remain planned.

- [ ] Bundle and validate a working HEVC decoder, including libde265 where used,
  in macOS, Windows, and Linux release packages.
- [ ] Complete the HEIF/HEIC/HIF fixture matrix across embedded-thumbnail,
  full-frame, tile-grid, high-bit-depth, ICC, HLG/PQ, orientation, and damaged files.
- [ ] Evaluate true reduced-resolution HEVC decode only where the packaged backend
  supports it and measurements beat the current embedded-preview path.

### Secondary-format reliability

- [ ] Implement and validate a TIFF preview backend on Windows and Linux. The
  current macOS implementation generates cache-compatible JPEG through ImageIO;
  the other platform stubs report that no native decoder is available.

### Metadata reliability

- [ ] Add conflict detection for source/sidecar changes during edits and define
  deterministic partial-failure reporting for multi-file writes.
- [ ] Complete native-versus-ExifTool round-trip fixtures for JPEG, HEIF/HIF, and
  the supported RAW families on every target platform.
- [ ] Evaluate a persistent ExifTool process only if measured compatibility-fallback
  or embedded-write workloads justify its lifecycle and packaging cost.

**Gate:** supported RAW and HEIF files show a fast cached representation without
depending on undocumented host capabilities, full-detail work never blanks the
existing preview, metadata edits preserve unrelated XMP, and every packaged native
dependency passes the release fixture matrix.

## Phase 3: Product and Release Readiness

### Product hardening

- [ ] Complete keyboard and screen-reader navigation, focus management, contrast,
  reduced-motion behavior, and platform accessibility audits.
- [ ] Add crash/session recovery for interrupted indexing, cache writes, metadata
  writes, and multi-file operations.
- [ ] Turn the application-level performance harness into a repeatable release
  gate with maintained baselines for folder open, cold preview, warm loupe, and
  full-detail diagnostics.

### Distribution

- [ ] Validate native dependency licenses, notices, codec availability, and
  relinking obligations for each release artifact.
- [ ] Sign and notarize macOS builds, sign Windows installers, and validate Linux
  packages on supported distributions.
- [ ] Validate the optional ExifTool capability flow under platform signing,
  quarantine, offline, invalid-download, upgrade, and rollback conditions.
- [ ] Run clean-install, upgrade, damaged-cache, missing-codec, and uninstall
  smoke tests on macOS, Windows, and Linux.

**Release gate:** all Phase 1 and Phase 2 gates pass on the declared support
matrix, signed installers pass clean-machine smoke tests, and user-owned photos
and sidecars remain recoverable across failure paths.

## Long-term: Native Image Presentation

Planned on 2026-09-13; implementation has not started. This is a future architecture
project, not an additional blocker for the current release. See the
[native image presentation plan](tasks/native-image-presentation-plan.md).

- [ ] Rebaseline the current RAW path without added sharpening, separating decode,
  delivery, GPU presentation, background encoding, and process/GPU memory.
- [ ] Prototype shared WIC pixels into a WebView2 Canvas/WebGL surface, avoiding
  JPEG encoding and decoding solely for foreground display.
- [ ] If the shared-pixel route remains insufficient, compare a native D3D image
  surface with WebView controls and record an adopt/stop decision in an ADR.
- [ ] Integrate the selected route with loupe interaction, cancellation, generations,
  resource leases, bounded background persistence, and runtime capability fallback.
- [ ] Validate packaged builds and regression gates; evaluate other platforms
  independently. Keep the existing camera-JPEG fast path and validated color behavior.

**Proposed gate:** for 24 MP SDR on fixed baseline hardware, native decode completion
to actual full-image presentation reaches median ≤100 ms / P95 ≤200 ms, with at
least 50% median improvement over the remeasured delivery baseline. These are targets,
not measured results; correctness, bounded memory, and browsing budgets must also pass.
