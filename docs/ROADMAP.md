# OxyViewer Roadmap

Status markers: `[x]` complete, `[~]` active, `[ ]` planned.

## Phase 0: Engineering Foundation

- [x] Cargo and pnpm workspaces
- [x] Domain contracts, structured errors, logging entrypoint, and i18n
- [x] Tauri 2 application shell and capability policy
- [x] CI definitions for macOS, Windows, and Linux
- [x] Architecture, format, performance, and licensing records
- [x] macOS debug desktop binary build and startup smoke test
- [~] Validate signed installers on all target platforms

**Gate:** passed locally on macOS on 2026-06-10. `cargo test --workspace`,
strict Clippy, frontend type checking/tests/build, and a debug Tauri desktop
build all pass. Cross-platform CI and signed installers remain release work.

## Phase 1: Instant Folder Browsing

- [x] Native folder picker and paged non-recursive directory scan
- [x] Grid/list/loupe workspace, search, type filters, and sort controls
- [x] Inline folder-name search with ancestor-preserving tree results
- [x] Multi-selection model and keyboard navigation
- [x] Safe rename/copy/move/trash service contracts
- [ ] Filesystem watcher and streamed `folder_delta` events
- [x] TanStack Virtual integration for grid and list rendering
- [ ] 100k-item generated-directory benchmark and performance tuning

**Gate:** first page from a local 100k-file directory begins rendering within
300 ms on the reference machine. Functional browsing is complete; this
performance gate has not yet been measured.

## Phase 2: Image Pipeline and Loupe

- [~] Media crate contracts, JPEG preview foundation, and RAW preview pipeline
- [~] Bundle and integrate LibRaw and libheif on all target platforms; libheif
  full-detail integration is implemented, while packaged HEVC decoder
  validation remains
- [~] Priority scheduler, cancellation, thumbnail cache, and custom protocol — unified priority scheduler (ADR 0005) covers ordering + abort-before-start; full cooperative cancellation is still open
- [x] Color-managed progressive loupe and filmstrip previews — unified JPEG + ICC cache (ADR 0005)

### Milestone RAW-1: Reliable RAW Display

- [x] Pin and vendor LibRaw 0.22.2 with a reproducible in-repository build
- [x] Read RAW dimensions through LibRaw
- [x] Extract the embedded RAW preview first
- [x] Fall back to half-size LibRaw development when no usable preview exists
- [x] Cache 512 px thumbnails and 4096 px loupe previews separately
- [x] Bound RAW decoding to one thumbnail and one loupe task concurrently
- [x] Keep macOS Quick Look as a final compatibility fallback
- [x] Pass a real camera RAW smoke test on macOS
- [ ] Validate ARW, CR2, CR3, NEF, DNG, RAF, RW2, and ORF fixtures
- [ ] Validate LibRaw builds and RAW preview behavior in Windows and Linux CI
- [~] Add full priority scheduling and request cancellation — the Rust-owned projection queue plus backend `DecodeGate` landed in ADR 0008; consumer cancellation does not discard accepted cache work, and cooperative mid-decode cancellation is still outstanding

### Milestone RAW-2: macOS Native Full-Size Rendering

- [ ] Benchmark Core Image `CIRAWFilter` on the RAW fixture matrix for cold
  full-size rendering, peak memory, output dimensions, orientation, color, and
  100% detail before selecting it as a production backend
- [ ] Add a macOS-only Core Image adapter in `oxy-media`; keep Tauri commands
  thin, perform rendering off the UI/async thread, reuse a bounded `CIContext`,
  and avoid unnecessary full-frame copies between Core Image and Rust
- [ ] Preserve the existing fast path: use the near-full-size embedded JPEG
  when it covers at least 90% of the RAW dimensions; otherwise prefer Core
  Image for macOS full-detail rendering and fall back to LibRaw development
- [ ] Keep Windows and Linux on the bundled LibRaw backend; Core Image must not
  become a requirement for 512/4096 previews or reduce portable RAW coverage
- [ ] Define the macOS output contract for EXIF orientation, working/output
  color spaces, ICC data, SDR tone mapping, and the current 8-bit JPEG cache
- [ ] Give Core Image its own cache/backend version and report the selected
  backend, decoder version, timing, and fallback reason through preview
  diagnostics
- [ ] Keep the 4096 preview visible if Core Image initialization or rendering
  fails, then verify automatic LibRaw fallback with fixture-backed tests

**Gate:** on supported macOS versions, Core Image full-detail rendering is used
only after the embedded-preview fast path, meets the measured fidelity and
resource budgets, and falls back to LibRaw without blanking or delaying the
already-visible loupe preview. Other platforms and unsupported macOS RAW files
retain the existing portable behavior.

### Milestone HEIF-1: Full-detail and Color-managed Display

- [x] Decode the primary image through libheif after progressive previews
- [x] Preserve high-bit-depth pixels through SDR conversion and 16-bit PNG cache
- [x] Convert ICC profiles and map HLG/PQ inputs to SDR
- [x] Bound full-detail HEIF decoding to one task
- [x] Keep an 8192 px macOS Quick Look compatibility fallback
- [ ] Bundle and validate libde265 for macOS, Windows, and Linux releases

### Milestone HEIF-2: HEIF Decoding Performance

- [x] Scale decoded image in libheif before expensive pixel processing (Image::scale)
- [x] LUT-accelerated HDR tone mapping (transfer function + sRGB gamma lookup tables)
- [x] SIMD-friendly unpack_rgb: split 8/16-bit paths, iterator-based batch processing
- [x] Confirm libheif parallel tile decoding enabled by default in v1.23
- [x] Preview cache write path: use JPEG instead of PNG for non-full-detail previews
- [x] Replace global Mutex decode locks with per-file granular locking
- [x] Pass DecodingOptions with explicit thread counts to leverage multi-core HEVC decode
- [x] Use embedded HEIF thumbnails first, including undersized thumbnails as a progressive first stage
- [x] Use an 8-bit RGB fast path for preview JPEGs while retaining color-managed high-bit-depth full detail
- [x] Use direct ImageIO full JPEG on macOS; cache a full JPEG after progressive HEIF tile display on Windows/Linux
- [x] Profile end-to-end decode pipeline; establish performance regression budget
- [ ] Evaluate true reduced-resolution HEVC decode when supported by the bundled decoder

**Gate:** a supported RAW file displays a cached thumbnail and loupe preview
without depending on operating-system RAW support. Unsupported or damaged RAW
files return a concrete LibRaw and fallback error instead of failing silently.

## Phase 3: Metadata Workflow

- [x] Sony ARW/JPEG/HEIF shooting-focus overlay with persisted and temporary controls
- [x] Rating/color contracts, RAW sidecar policy, list filters, and ExifTool worker boundary
- [ ] Bundle persistent ExifTool worker
- [~] Rating/color read/write round trips and batch editing; debouncing and conflict handling remain

## Phase 4: Library and File Management

- [x] SQLite WAL library roots, rebuildable directory/file cache, and FTS schema
- [~] Low-priority indexing, cross-folder filters, and live search results — serialized background indexing and live FTS refresh now cover every directory inside one loaded root; cross-root and metadata-aware indexed filters remain
- [ ] Complete file-operation dialogs, undo journal, and recovery

## Phase 5: Release Hardening

- [ ] Native dependency packaging, signing, notarization, and installers
- [ ] Accessibility, keyboard efficiency, crash recovery, and benchmarks
- [ ] Complete fixture matrix and third-party notices
