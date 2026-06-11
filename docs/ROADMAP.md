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
- [ ] Priority scheduler, cancellation, thumbnail cache, and custom protocol
- [ ] Color-managed progressive loupe and filmstrip previews

### Milestone RAW-1: Reliable RAW Display

- [x] Pin and vendor LibRaw 0.22.1 with a reproducible in-repository build
- [x] Read RAW dimensions through LibRaw
- [x] Extract the embedded RAW preview first
- [x] Fall back to half-size LibRaw development when no usable preview exists
- [x] Cache 512 px thumbnails and 4096 px loupe previews separately
- [x] Bound RAW decoding to one thumbnail and one loupe task concurrently
- [x] Keep macOS Quick Look as a final compatibility fallback
- [x] Pass a real camera RAW smoke test on macOS
- [ ] Validate ARW, CR2, CR3, NEF, DNG, RAF, RW2, and ORF fixtures
- [ ] Validate LibRaw builds and RAW preview behavior in Windows and Linux CI
- [ ] Add full priority scheduling and request cancellation

### Milestone HEIF-1: Full-detail and Color-managed Display

- [x] Decode the primary image through libheif after progressive previews
- [x] Preserve high-bit-depth pixels through SDR conversion and 16-bit PNG cache
- [x] Convert ICC profiles and map HLG/PQ inputs to SDR
- [x] Bound full-detail HEIF decoding to one task
- [x] Keep an 8192 px macOS Quick Look compatibility fallback
- [ ] Bundle and validate libde265 for macOS, Windows, and Linux releases

**Gate:** a supported RAW file displays a cached thumbnail and loupe preview
without depending on operating-system RAW support. Unsupported or damaged RAW
files return a concrete LibRaw and fallback error instead of failing silently.

## Phase 3: Metadata Workflow

- [~] Metadata contracts, RAW sidecar policy, and ExifTool worker boundary
- [ ] Bundle persistent ExifTool worker
- [ ] Read/write round trips, debounced writes, conflicts, and batch editing

## Phase 4: Library and File Management

- [~] SQLite WAL library root and FTS schema foundation
- [ ] Low-priority indexing, cross-folder filters, and live search results
- [ ] Complete file-operation dialogs, undo journal, and recovery

## Phase 5: Release Hardening

- [ ] Native dependency packaging, signing, notarization, and installers
- [ ] Accessibility, keyboard efficiency, crash recovery, and benchmarks
- [ ] Complete fixture matrix and third-party notices
