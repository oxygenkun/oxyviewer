# Performance Budgets

Reference budgets are measured on a local SSD with a release build. The
end-to-end regression harness that enforces these budgets is described in
[PERF_E2E.md](PERF_E2E.md) and run via `pnpm perf:e2e`.

| Interaction | Target |
| --- | --- |
| First page from a 100k-file directory | Begin rendering within 300 ms |
| Warm cached loupe preview | Under 150 ms |
| Cold selected-image preview | Under 800 ms |
| RAW 100% display fidelity | Same-file paired capture; mean RGB within 3/255 per channel and no visible detail loss |
| Full-resolution RAW development | Measured separately; preview remains visible |
| Search/filter response over loaded page | Under one animation frame |
| Main-thread scroll work | No long task above 50 ms |

## Verification Log

- 2026-09-10: Removed the scroll-idle resource gate for grid/list/filmstrip and
  expanded overscan to 6 rows / 16 items / 12 items respectively. Preview workers
  and the shared decode gate use available logical CPU count (8 on this Mac),
  with a selected-image reserve on multicore systems. UI decoded images and HEIF
  completed presentation nodes share a 1024-entry / 512 MiB LRU; cached-only
  native pins are capped at 256 so tiny thumbnails cannot exhaust the native
  registry's 512 slots. Immutable resource status updates retain decoded pixels.
  Real HIF return testing found and fixed both a 180–220 ms full-canvas copy and
  a parent effect that overwrote a cache hit with the loading state. Returns now
  reattach the retained presentation node and reset selection state before commit.
  Three isolated release/WebView runs of `navigation-cache-jpeg` verified 12
  cached returns at 4–9 ms (median 5.5 ms). `navigation-cache-hif` verified 24
  returns across sharpened canvas and unsharpened artifact presentations at
  6–75 ms (median 8 ms), with an explicit memory-cache-hit requirement.
  Earlier cold/warm JPEG first-preview median/P95 was 53/66 and 60/61 ms;
  cold HIF was 40/49 ms, with first tile at 374/507 ms.
  Three continuous grid-scroll runs over 600 JPEGs completed every stopped
  viewport in 27–59 ms. The small-grid pressure test painted all 600 images,
  peaked at 277 native entries, and settled at 61 entries for 61 mounted images;
  the 80-HIF rapid-selection test revisited the last eight full images and
  settled at 28 entries for 28 mounted images. Both passed cache-maintenance
  protocol-read checks. These are local fixture results, not NAS measurements.
  Final unlocked release verification passed three `filmstrip-scroll-hif` runs:
  all 36 moving samples had all 17 visible thumbnails painted, with each sweep
  taking 644–710 ms and whole-viewport readiness confirmed 27–33 ms after stop.
  Three final 600-JPEG grid runs had 33/36 moving samples fully painted (the
  minimum was 31/32), with all stopped viewports ready in 27–30 ms. Grid sweeps
  took 1.62–2.72 seconds; these samples establish continued loading during
  scrolling, not a frame-rate guarantee. The 100k first-page budget is not
  claimed here.

- 2026-09-09: On the reported 383-entry SMB HIF folder, three release/WebView continuous-scroll
  runs per implementation measured stop-to-whole-viewport readiness. Median times at the three
  stops changed from 1067/889/820 ms to 397/391/394 ms. The fixes bound the normal Sony prefix read
  to 256 KiB (2 MiB fallback), remove generation-read contention with background publication, and
  avoid unchanged manifest rewrites during prune. Worker count and the 160 ms scroll-idle gate
  are unchanged. App artifact caches were isolated/cold; OS/NAS caches were not cleared.
  Reproduce with `--folder <path> --grid-scroll --cold-cache --runs 3`;
  [case details and validation limits](tasks/nas-hif-grid-latency.md).

- 2026-09-08: Final media-cache review fixes were measured with a rebuilt macOS release binary,
  three isolated packaged-WebView runs per route, and repository fixtures. Median/P95 first-preview
  times were JPEG cold 86/89 ms and warm 70/72 ms; HIF cold 59/69 ms; ARW cold 61/69 ms and warm
  44/51 ms. The explicitly named sharpened `warm-loupe-hif-tiles` route rejected non-cache backends;
  after a successful warmup and managed-artifact check, all three runs used `cachedArtifact`: first
  preview 45/53 ms, first tile 244/252 ms, and all tiles 564/572 ms. The 150 ms warm preview gate passes,
  but tile completion has no 150 ms claim. Warmup failures/missing marks now invalidate measurements.
  The 100k scenario was corrected to clear isolated data before every sample: first-page paint was
  753/759/763 ms (median 759, P95 763), comprising 645/646/659 ms to first-page IPC return plus
  104/108/113 ms from return to paint. A later instrumented run, after batching scanner progress,
  deferring rebuildable snapshot serialization/persistence behind bounded coalesced epoch/revision
  fencing, and caching name sort keys, measured 519/498/465 ms (median 498, P95 519). Its median/P95
  split was 84/110 ms enumeration, 330/341 ms accurate size/mtime attributes, 0/0 ms response-path
  snapshot serialization, 0/0 ms snapshot persistence/enqueue, 10/11 ms sorting, 435/452 ms native,
  437/454 ms IPC, and 57/79 ms return-to-double-rAF paint. The gate remains failed without weakening
  the 300 ms paint metric or the complete-summary contract. The intermediate synchronous-persistence
  measurement is documented in `tasks/media-cache-redesign.md`; it is paired implementation evidence,
  not a historical baseline claim. Windows/Linux lock, rename/sharing and packaged protocol behavior remain
  unverified environment gates.

- 2026-09-08: Media-cache v2 application acceptance used a freshly built macOS release binary and
  runner-owned app data/cache under `tests/perf/.reports/.runtime/`; no normal user database/cache was
  read or cleared. Final single packaged-WebView samples measured cold/warm JPEG at 61/57 ms, cold HIF
  at 62 ms to first preview (461 ms first full tile), and cold/warm `DSC00529.ARW` at 62/39 ms. A warm
  HIF run reached first preview in 38 ms and first/all tiles in 336/491 ms, but timed out waiting for the
  scenario's `image:loaded@full` mark, so its 150 ms full-artifact gate is not passed. The 100k
  synthetic-directory first-page sample was 1039 ms and remains above the 300 ms budget. These
  are one-run functional samples, not a new baseline or P95. Windows/Linux protocol/CSP and cache
  lock/rename/sharing matrices remain unverified. The custom-protocol materialization budget covers
  Rust's concurrent `Vec<u8>` construction only; Tauri owns the response body after return and exposes
  no response-drop accounting hook.

> `tests/fixtures/DSC00449.HIF` 等真实照片 fixture 不再纳入 git 历史；获取方式与校验和见
> `tests/fixtures/README.md`，缺失时相关测试会自动跳过，可用 `OXY_HIF_FIXTURE` 指定路径。

- 2026-09-05: Phase D 的 macOS release 后测使用 `tests/fixtures/DSC00449.HIF`、隔离临时冷缓存和
  `heif_display_bench ... 5 scroll`。经表示事实识别的 Sony 160px JPEG 五次后端总耗时为
  16/5/3/11/5 ms，端到端函数样本 median 7.90 ms、min 6.80 ms；每次产物为 120×160、约 8.1 KiB。
  这是 D 后单平台样本，不是与 D 前相同构建的成对比较，也不含 Tauri/WebView paint，不能据此
  宣称跨平台或完整交互无回归。generic HEIF 512/4096 和 RAW/color fixture 矩阵仍需补测。

- 2026-09-05: Directory browsing persists independent complete snapshots and
  reuses them across process restarts without a completed root index or a
  filesystem stat. Successful background validation atomically replaces the
  saved list; failure retains it. Explicit invalidation fences late results
  and writes a tombstone so an obsolete index cannot reseed it. Pagination
  pins a revision; the previous immutable list survives background replacement.
  Foreground list/visible-preview work pauses index and snapshot scans at entry
  boundaries and directory-tree work at job boundaries. Only one nearby/preload
  decode starts at a time. Running OS I/O and decodes remain non-preemptive.
  One-pass file/XMP pairing and Windows DirEntry attributes avoid per-photo
  sidecar probes and path stats. Paging sorts references and clones only its
  returned summaries. Size/mtime remain accurate; temporary filesystem-order
  pages are not published.

  Read-only debug backend measurement on a registered NAS directory with 2,164
  photos, using isolated temporary SQLite: cold list plus first-page sorting
  329 ms (enumeration 280 ms, attributes 7 ms); reopened snapshot plus sorting
  29 ms; same-process paging 9 ms. These are not full startup, UI paint, cold
  NAS/server-cache measurements or qualification of the 100k-file release
  budget. Reproduce: `cargo run -p oxy-library --example browse_latency -- <directory>`.
  Regression coverage includes restart/offline, empty snapshots, incomplete
  indexing, coalesced reads, changed files/XMP, invalidation races and pinned
  paging. `scripts/browse-startup.browser.js` verifies six App/IPC UI behaviors
  with controlled responses, including loading counts and snapshot/offline states.

  `window.__oxyBrowseDiagnostics` exposes the last 200 real startup records in
  the WebView console: workspace discovery/readiness, native cache/enumeration/
  attributes/sorting/elapsed times, first-page IPC return and the next frame
  after React commit. Repeated progress in the same stage is coalesced so a
  slow scan does not evict its startup milestones. That frame does not prove thumbnail pixels have decoded
  or painted. Native intermediate progress is throttled to ten events/second.

- 2026-09-05: Workspace restoration submits the saved active root first and
  publishes each opened root immediately, without waiting for other roots.
  Pending and failed roots have individual sidebar states. While the saved
  active root is pending, other ready roots can be selected without an automatic
  switch away from the saved directory; late results do not override that
  selection. Import order is preserved and drag reordering is disabled until
  restoration finishes. Deferred-promise tests cover slow/offline roots and
  removing a ready root while another is pending. TypeScript checks, 117
  frontend tests and the production build pass. This removes the all-roots
  publication barrier; it does not prioritize native I/O, change filesystem
  scanning or establish NAS first-page timing. Persistent directory snapshots,
  cold-scan costs and foreground/background contention remain separate steps.

- 2026-09-05: Disk-backed libraries now use separate read-only WAL connections
  for browsing/tag queries and resource-projection cache lookups. These readers
  do not acquire the writer mutex, and multi-statement asset/directory queries
  use a read transaction to keep completion checks, counts and results in one
  committed snapshot. Index directory/asset writes stop at 256 rows or an 8 ms
  cooperative time slice and fairly hand off the writer mutex after committing.
  The time slice cannot preempt a SQL statement, commit or WAL checkpoint; final
  generation cleanup remains atomic. In-memory libraries retain a single
  connection. Concurrency tests hold an uncommitted writer transaction while
  browsing completes and verify that cache reads also bypass the browsing lock.
  The manual on-disk debug benchmark
  `cargo test -p oxy-library wal_interaction_latency_during_large_index -- --ignored --nocapture`
  sampled 200 foreground interactions during 50k synthetic background asset
  writes: combined visible-page/cache reads P95 0.56 ms / max 1.96 ms, resource
  revision writes P95 35.56 ms / max 53.64 ms. This is backend contention
  coverage, not a release-build UI P95 measurement. Preview/metadata enqueue
  still holds queue state during projection writes; fair writer handoff reduces
  index-induced waiting but does not eliminate those queue-lock dependencies.

- 2026-09-05: Asset indexing now maintains an ordinary SQLite key table mapping
  `(root_path, path)` to a stable FTS rowid. New and changed assets replace FTS
  rows by rowid instead of scanning the entire FTS table on its `UNINDEXED`
  path columns for every asset while holding the shared library connection
  lock. Existing caches migrate their FTS rowids transactionally without a
  filesystem rescan; root removal and stale-generation cleanup remove both
  search records and keys. Migration, changed search terms, overlapping roots,
  and stale-file cleanup have regression coverage. The manual Rust benchmark
  `cargo test -p oxy-library large_library_index_batch_latency -- --ignored --nocapture`
  measured the next 256 complete asset writes at 7.2 / 7.5 / 8.1 ms with
  1k / 10k / 50k records in a debug-build in-memory database. The prior SQL
  microbenchmark took 71 / 688 / 3457 ms for only the 256 missing-record deletes
  at those sizes. These are backend microbenchmarks, not release UI latency
  qualification. Shared-connection contention and whole-directory asset
  collection still warrant measurement on real large folders.

- 2026-09-05: The filmstrip now virtualizes the complete summary range instead
  of mounting every loaded asset beside one large unloaded spacer. A jump into
  an unloaded range renders bounded per-item placeholders and requests sequential
  250-item pages until the loaded range covers the viewport; page completion
  retriggers this check without another scroll event. The browser regression in
  `scripts/filmstrip-pagination.browser.js` mounts the real Loupe with 1,473
  synthetic summaries and delayed pages, jumps across an unfinished page and
  then to the end, and keeps mounted asset buttons below 40 (15 near item 510
  and 10 at the end in the reference Chromium run). Page commits no longer
  rebuild a selection-priority map or reconcile a background schedule for every
  loaded filmstrip item; visible and neighboring items retain their scoped work.
  Metadata enrichment now follows only visible virtual items and commits each
  returned batch to the Zustand mirror in one update instead of requesting a
  whole page and copying the growing record map once per asset. Page and metadata
  projections use React deferred values so scroll input remains urgent while a
  newly returned page is incorporated. Opening does not eagerly fetch additional
  pages. This verifies pagination and bounded DOM behavior in
  Chromium, not native media latency or the release-build timing budgets above.

- 2026-09-04: Fast scrolling now keeps the cheap viewport schedule current
  while filesystem reads and WebView image decode remain paused. Cancelling an
  off-screen React Query also releases its Rust request consumer and removes
  work that has not started and has no remaining consumer, instead of leaving
  a stale preload request in front of a newly visible cold ARW region. Native
  decode that has already begun remains non-preemptive and may finish into the
  rebuildable cache.

- 2026-09-04: RAW thumbnails now preserve LibRaw's size-selected embedded JPEG
  bytes instead of decoding, resizing to exactly 512 px, and re-encoding each
  file. This aligns cold ARW grids with the HEIF embedded-preview fast path;
  non-JPEG embedded images and missing-preview development remain fallbacks. A
  Windows release benchmark of `DSC02905.ARW` improved the 512 request from
  187.6 ms to 12.3 ms. Three real-App cold-cache runs against a 106-file ARW
  folder painted the jumped-to last viewport in 554 ms median / 564 ms P95,
  down from the reproduced 4.11 s. Three cold loupe runs painted the full
  embedded JPEG in 278 ms median / 290 ms P95.

- 2026-09-07: Windows release builds now install the pinned vcpkg libheif port
  with only its `core` feature. The application discovers HEIF/HEIC/HIF assets
  and uses libde265 for the required HEVC compatibility decode; it does not
  currently discover AVIF assets, so neither the AOM AV1 codec nor the default
  x265 HEVC encoder belongs in the package. This removes both codec builds and
  the AOM static-link workaround from the release path. CI restores the vcpkg
  binary archive before installation and saves it immediately afterward, so a
  later Cargo or packaging failure does not discard the completed native build.

- 2026-09-04: Grid and list rendering now enter a presentation-only phase
  during active scrolling. Virtual rows, cheap summaries, placeholders, and
  already-decoded browser images continue painting, while new filesystem image
  loads, WebView decodes, and viewport preview intents wait until the viewport
  has been idle for 160 ms. Folder open, preview cache lookup/enqueue/wait, and
  preview scheduling commands now move their blocking filesystem, SQLite, and
  queue-lock work onto Tauri's blocking pool; the command executor no longer
  performs those operations inline. The Rust preview data path remains the
  persisted resource-projection table plus SSD artifacts, a four-tier
  coalescing pending queue (`loupe`, `visible`, `nearby`, `preload`), an active
  request map, and two decode workers.

- 2026-09-04: Grid, list, and the loupe filmstrip now rank thumbnail work around
  the current selection when it is visible, otherwise from the viewport center.
  When an item leaves the overscan area,
  React Query stops waiting for it and the Rust-owned pending task is demoted
  to `preload` rather than cancelled, so it can still populate the cache after
  current-screen work. Filmstrip preview warming likewise stops its foreground
  wait and moves on to the new viewport while the former candidate continues
  in the background. Priority changes use a lightweight, bidirectional queue
  update rather than a duplicate blocking preview request. Two bounded preview
  workers keep one slow, already-started decode from stalling the entire visible
  screen; decoder-specific gates continue to enforce their own concurrency
  limits. Frontend schedule snapshots are animation-frame coalesced, content
  deduplicated, and limited to one viewport IPC per 50 ms. Background ordering
  is constructed after a 150 ms debounce and limited to one IPC per 200 ms;
  each scope permits only one bridge request in flight.

- 2026-09-04: Adopted Rust-owned versioned resource projections for metadata
  and image state (ADR 0008). Selected, visible, filter, and background work
  share stale-result rejection and priority semantics. Pending and in-flight
  requests for one resource revision coalesce in Rust; frontend stores only
  render mirrors and WebView preload results rather than competing authorities.
  Metadata source observation stays off the UI thread, and directory listing
  remains cheap. Accepted projections and their WAL-ordered observation revisions
  are persisted in SQLite, allowing restart cache hits and preventing late results
  from another database connection from overwriting newer state. Migration
  measurements continue to enforce the existing first-page and selected-preview
  budgets.

- 2026-09-04: Restored progressive HEIF loupe tiles on a Windows cold full-image
  cache miss after the single full-JPEG display path regressed perceived loading
  to about two seconds. The embedded JPEG remains visible immediately; the first
  full decode paints tiles as they arrive and still writes the source-derived
  JPEG for later warm loads. Three Windows release backend runs measured a
  569 ms median first tile and 797 ms median completion. macOS retains its
  separately qualified ImageIO source-to-full-JPEG path.
- 2026-09-03: Loupe filmstrip visibility now drives a sequential frontend
  warmup of the same first-stage render query used by the main image, followed
  by explicit WebView image decode. Every frontend image queue shares one
  selection-centered ordering: the active image, its nearest right neighbor,
  nearest left neighbor, remaining images to the right, then remaining images
  to the left. Full RAW/HEIF work
  remains selection-driven, and a prepared image becomes the loupe's visible
  layer immediately instead of flashing through the empty fallback.
- 2026-09-04: Review flags join the fingerprinted metadata projection without
  adding work to folder open. Visible-item enrichment now publishes flag,
  `xmp:Rating`, and `xmp:Label` together from the same parse.
- 2026-09-03: Grid metadata enrichment and rating/color filtering now share a
  fingerprinted in-memory projection cache. Switching on a metadata filter
  reuses rating and color labels already parsed for visible assets and parses
  only cache misses; metadata edits and explicit directory refreshes invalidate
  the affected entries. Metadata filtering publishes matching assets after each
  32-item parse batch, so grid, list, filmstrip, and loupe update progressively
  instead of waiting for the complete directory pass.
- 2026-09-03: A same-machine, same-fixture release E2E sweep tested macOS
  ImageIO full-resolution HEIF publication at 512 / 1024 / 2048 / 4096 / 8192
  px per RGBA tile, with ten measured runs per size after warmup. Median
  first-tile times were 342.5 / 354 / 356.5 / 392.5 / 434 ms; p95 values were
  356 / 385 / 403 / 406 / 502 ms. Median all-tile times were 446.5 / 444.5 /
  430 / 451.5 / 434 ms; p95 values were 467 / 472 / 482 / 477 / 502 ms.
  Backend-only medians improved monotonically from 412 ms at 512 px to 324.5
  ms for the single 8192 px response, but protocol transfer and Canvas paint
  erased that gain. Since selected-image responsiveness prioritizes the first
  useful full-resolution region and stable tail latency, macOS retains 512 px
  center-first tiles. Windows retains its separate 1024 px fallback and its
  faster six-source-grid JPEG path. Larger macOS tiles are not a proven preview
  acceleration for this fixture.
- 2026-09-03: A follow-up macOS payload experiment compared the retained RGBA
  transport with quality-90 JPEG tiles encoded directly to memory by ImageIO.
  Ten release backend runs per variant found that 512 px RGBA published 124.9
  MiB in 44.02 ms median, while JPEG reduced the payload to 0.6 MiB but raised
  completion to 106.40 ms. JPEG at 1024 and 2048 px published about 0.5 MiB in
  96.35 and 93.14 ms respectively; median first-tile times were 28.61 and
  35.18 ms versus 26.26 ms for RGBA. A pure-Rust JPEG encoder was substantially
  slower still. The existing RGBA E2E measurements leave only about 35 ms
  between backend completion and all-tile paint, less than ImageIO JPEG's
  roughly 49-62 ms added backend cost even before WebKit decodes the JPEGs.
  JPEG therefore provides a major byte-volume reduction but is not retained as
  the macOS default for this fixture. Attempts to rerun the packaged E2E with
  the experimental encoder were blocked by the independent harness startup
  issue where the WebKit content process terminated before writing a report.
- 2026-09-03: A second follow-up tested the missing non-tiled case: ImageIO
  decoded and sharpened the full `DSC00449.HIF`, encoded one quality-95 JPEG,
  and published it as a single session payload. Across ten release backend
  runs, the JPEG payload was about 0.5 MiB and total publication measured 81.05
  ms median (79.89 ms minimum), including a temporary-file write and read
  representative of a persistent cache. The same-machine 512 px RGBA control
  published 140 tiles totaling 124.9 MiB in 45.35 ms median (42.65 ms minimum),
  with first-tile availability at 27.26 ms median. One full JPEG therefore cuts
  the payload by roughly 250x but adds about 36 ms before any full-resolution
  pixels can be drawn. The packaged E2E again terminated its WebKit content
  process during application startup, before HEIF decode, so browser fetch,
  JPEG decode, and paint latency were not measured and must not be inferred
  from these backend results. The experiment supports a warm persistent JPEG
  cache, not replacing the cold progressive RGBA path.
- 2026-09-03: Sony HIF grid/list/filmstrip thumbnails now use the camera's
  embedded 160x120 MJPEG item directly instead of requesting a rigid 512 px
  preview and launching FFmpeg for the 1664x1088 HEVC auxiliary image. The
  `SHIF`-gated fast path scans only the first 2 MiB, validates the JPEG, and
  injects EXIF Orientation so WebView2 rotates the original compressed bytes
  without pixel re-encoding. On Windows the `thumbnail` and loupe `preview`
  levels both reuse this 160 px artifact; only full tiles begin when the image
  is opened. Ten cold-cache
  release runs of `DSC00449.HIF` measured 12.6 ms median and 10.5 ms minimum,
  versus about 383 ms median for the former FFmpeg 512 px path. Because this
  path runs before the HEVC decode gate, sidebar scrolling is no longer
  serialized behind full-image work.
- 2026-09-03: Replaced pixel-sized progressive UI stages with the semantic
  `thumbnail → preview → full` render graph (ADR 0006). The IPC now carries a
  `RenderLevel` only; `oxy-media` owns platform/format decoder and size policy.
  Windows Sony HIF `thumbnail` and `preview` resolve to the same embedded
  160×120 artifact and React Query cache key, while `full` remains the tile
  session. A loupe can raise a shared pending thumbnail request to foreground
  priority in place, preserving scheduler ordering without a duplicate decode.
- 2026-09-03: Direct Windows process/GPU-engine sampling of Sony Imaging Edge
  Viewer while switching among three 7008x4672 Sony HEIF 4:2:2 files found a
  CPU decode path on the reference workstation. `Viewer.exe` loaded Sony's
  private `sonyhevd.dll` plus D3D11, but image switches drove about 254-318%
  process CPU while the only attributable GPU activity was 0.08-0.10% on the
  3D engine. No Video Decode, Video Processing, or Compute engine activity was
  recorded. D3D11 is therefore used for presentation on this configuration,
  not as evidence of HEVC hardware decode. Matching Sony's interaction latency
  should prioritize independent camera previews, parallel tile decode, cache
  reuse, and avoiding full-frame RGBA intermediates; GPU decode remains an
  optional, fixture-qualified backend rather than a requirement.
- 2026-09-02: Windows HEIF progressive preview now uses a sufficiently large
  independent camera-rendered stream through FFmpeg before falling back to a
  full libheif decode. Three cold, packaged-app E2E runs of `DSC00449.HIF`
  measured 542 ms median to paint the 512 px preview (524 ms minimum), down
  from a 2.74 second decoder median and effectively matching the recorded
  537 ms macOS median for the same fixture. With the full-resolution session
  starting in parallel (and yielding to the preview through the decode gate),
  a later three-run cold check measured 563 ms median and 597 ms p95. The
  isolated release preview decoder measured 345 ms median with a warm
  executable and 451 ms on its first invocation.
  The 4096 compatibility path and full-resolution tile publication remain
  slower than macOS and are tracked separately; the interactive first-preview
  budget is now met on the reference Windows machine.
- 2026-09-02: Windows full-resolution HEIF no longer uses FFmpeg's slow
  `xstack` filter. FFmpeg decodes and color-converts the six source tiles in
  parallel, crops, rotates, and sharpens them independently, and now delivers
  the six source-grid components as high-quality JPEG tiles. This avoids a
  131 MB assembled RGBA frame and 35 raw WebView2 transfers. Five isolated
  release runs of `DSC00449.HIF` measured 643 ms median to the first tile and
  671 ms for all tiles, down from 2.15 and 2.54 seconds respectively and below
  the recorded 756 ms macOS all-tile result. The final three-run packaged-app
  E2E check measured 1098 ms median to first paint and 1146 ms to all tiles
  (702 ms backend median), down from 1206/2027 ms for the raw-tile path. An
  earlier three-run set reached 1018/1061 ms, showing some process-start
  variance. End-to-end is still above the recorded 505/756 ms macOS result,
  but full-image completion is now about 44% faster than the prior Windows
  implementation.
- 2026-09-02: A paired 100% display check opened the byte-identical
  `DSC02948.ARW` fixture in Sony Imaging Edge Viewer and OxyViewer. After
  viewport registration, Sony's sampled crop averaged RGB
  `157.66 / 132.79 / 118.30`; OxyViewer's near-full embedded-JPEG path averaged
  `158.52 / 134.48 / 120.68`. The per-channel mean difference stayed below
  3/255 and luminance structure correlation was 0.952. The comparison also
  caught a stale packaged app still serving an older 6240x4168 LibRaw-developed
  cache; the current package reports and displays the camera-rendered
  6192x4128 JPEG. Visual acceptance must therefore use the newly built app,
  the same source bytes, 100% zoom in both viewers, and an aligned image region.
- 2026-09-02: RAW loupe entry now requests the 4096 stage directly instead of
  first decoding and re-encoding a 512 px image. The full-detail stage reuses
  a camera-rendered embedded JPEG when both edges cover at least 90% of
  LibRaw's source dimensions, avoiding a CPU-heavy full demosaic during zoom.
  On the repository Sony fixtures, `DSC00529.ARW` exposed 7008x4672 in 13 ms
  and `DSC02948.ARW` exposed 6192x4128 in 11 ms; the former unconditional full
  development historically took 23.75 seconds.
- 2026-09-02: HEIF loupe zoom now uses the full-image dimensions rather
  than the 512 px placeholder's natural size. A displayed 100% therefore maps
  one source pixel to one CSS pixel and 400% is a true four-times pixel zoom.
  HEIF full-resolution display tiles also receive an optional mild luma
  unsharp mask, enabled as the default Standard setting to match Sony Imaging
  Edge Viewer edge definition without modifying source files or caches. macOS
  uses Accelerate/vImage for the full-frame convolution before tile slicing,
  so neighboring samples cross tile boundaries. Three Debug runs on the NAS
  `DSC00518.HIF` fixture measured 36-52 ms ImageIO decode, 42-49 ms sharpened
  tile publication, and 78-101 ms backend total.
- 2026-09-03: The active HEIF loupe path uses the embedded JPEG as a temporary
  base layer, then replaces it with one full-resolution JPEG converted directly
  from the source HEIF. Canvas tile events and `oxy-media://` tile transfers are
  no longer started by the frontend. On macOS, the full conversion stays inside
  ImageIO (`CGImageSource` to JPEG destination), so the former 125 MiB Rust RGBA
  re-encode is not part of cache generation.
- 2026-09-01: The former HEIF loupe tile path used the 512 px JPEG only as a temporary
  base layer. On macOS, HEIF preview JPEGs are encoded by ImageIO and
  the global decode permit is released after decode, before JPEG encode and
  cache sync. Full-tile publication converts the decoded image to RGBA once
  and copies contiguous rows into tiles. On the reference Mac against
  `<NAS_FIXTURE_DIR>/DSC00463.HIF`,
  the optimized Debug backend measured about 46 ms ImageIO decode and 31 ms
  tile publication, versus the previous in-app 308 ms and 1363 ms. A real
  cold loupe run painted its first tile in 505 ms and all tiles in 756 ms while
  grid/filmstrip thumbnail work was active. The former concurrent 4096 request
  (about 7.5 s including queue wait) is no longer issued.
- 2026-06-10: grid/list rendering uses TanStack Virtual and browser interaction
  checks passed for selection, search filtering, inspector updates, and loupe
  switching.
- 2026-06-11: RAW thumbnail extraction now selects the smallest embedded LibRaw
  preview that satisfies the requested size instead of always decoding the
  largest preview. Cache JPEG encoding also avoids an unconditional full-image
  RGB copy.
- 2026-06-11: RAW loupe previews now preserve a suitable embedded JPEG instead
  of decoding, resizing, and re-encoding it. Loupe rendering requests a 512 px
  preview first and upgrades to 4096 px after the first stage is available.
  Before this change, the measured ARW 512 px path was about 120 ms; HIF Quick
  Look took about 250 ms at 512 px and 590 ms at 4096 px.
  Release-build fixture results on the reference Mac:

  | Fixture | 512 px cold | 4096 px cold after 512 px | 4096 px warm |
  | --- | ---: | ---: | ---: |
  | `DSC00529.ARW` | 100 ms | 7 ms | 0.03 ms |
  | `DSC02948.ARW` | 85 ms | 11 ms | 0.03 ms |
  | `DSC00511.HIF` | 261 ms | 753 ms | 0.04 ms |
  | `DSC00526.HIF` | 226 ms | 666 ms | 0.04 ms |
- 2026-06-11: RAW loupe viewing now starts full-resolution LibRaw development
  after the progressive preview is available. Full development uses a separate
  cache and decode lane, does not count against the 800 ms cold-preview budget,
  and keeps the preview visible until the full-resolution JPEG has loaded. A
  release fixture run for `DSC00529.ARW` took 23.75 seconds and produced an
  output matching LibRaw's full reported dimensions.
- 2026-06-11: Full RAW loupe output now avoids full-strength FBDD pre-demosaic
  noise reduction, applies modest output sharpening, and renders zoomed images
  at their target CSS dimensions instead of scaling a fit-sized composited
  layer. This improves fine-detail inspection at high zoom.
- 2026-06-11: Selected HEIF/HIF images now request a full-detail stage after
  the 512 px and 4096 px progressive previews. The primary libheif image is
  decoded in a dedicated lane and cached as a color-profiled 16-bit SDR PNG;
  HLG/PQ inputs are tone-mapped to SDR. macOS uses an 8192 px Quick Look result
  when the packaged HEVC decoder rejects the stream, avoiding the previous
  unconditional 4096 px ceiling.
- 2026-06-12: HEIF preview generation now follows nomacs' fastest useful
  behavior: prefer container thumbnails before primary-image decode, accept an
  undersized embedded thumbnail as the immediate progressive stage, cache
  preview JPEGs, and serialize different preview sizes for the same source to
  avoid duplicate concurrent HEVC decodes. Preview JPEGs use a direct 8-bit
  RGB decode path; high-bit-depth color-managed processing remains isolated to
  the later full-detail stage. An experimental all-tile path was removed
  because it decoded every tile sequentially and did not reduce HEVC work.
- 2026-06-13: HEIF decode work is now globally single-flight with interactive
  priorities: the current loupe image first, visible grid/filmstrip thumbnails
  second, and nearby overscan thumbnails last. Grid and filmstrip HEIF
  thumbnails also use a cancellable priority queue, so requests that have not
  started are discarded or reprioritized as visibility changes instead of
  continuing after navigation.
- 2026-09-03: Active list filters no longer stop same-directory cache warming.
  A separate cheap, unfiltered paged query supplies hidden candidates to a
  sequential `preload` lane. It submits only one item at a time below nearby
  overscan priority, so loupe, visible, and nearby requests continue to jump
  ahead while non-matching images eventually warm in the background.
- 2026-06-14: libheif preview decoding now uses size-based thread limits. Grid
  thumbnails use one codec/library thread, while larger progressive previews
  receive modestly higher limits, preventing background browsing from
  saturating macOS CPU cores.
- 2026-06-14: macOS HEIF thumbnail, 4096 px preview, and full-resolution tile
  sessions now prefer the native ImageIO decoder and fall back to FFmpeg or
  libheif. ImageIO may use the platform media stack internally, but is reported
  as unknown acceleration until GPU use can be verified. The reusable
  `heif_display_bench` binary measures cold 512/4096 previews, first full tile,
  and all-tile publication. On the reference Mac, three release runs of
  `DSC00449.HIF` measured 537 ms median for a cold-cache 512 px preview and
  565 ms for 4096 px. After those preview runs had warmed the system decoder,
  full-resolution ImageIO decode took 9-27 ms, the first center tile arrived
  in 13 ms median, and all tiles were published in 74 ms median. The remaining
  measured costs are preview JPEG cache writing and frontend per-tile transfer.
- 2026-06-12: A clean release-mode libheif benchmark was added at
  `crates/oxy-media/src/bin/heif_decode_bench.rs`. On the Windows reference
  machine, `tests/fixtures/DSC00449.HIF` is 4672x7008, 10-bit, and has no
  embedded thumbnail. libheif 1.23.0 reports only the libde265 1.1.1 decoder.
  Full-resolution display-ready RGB8 decode measured 2.51 seconds median and
  2.30 seconds minimum; tight pixel copying added about 32 ms. One, default,
  and 24 codec threads performed nearly identically. The current libde265
  backend therefore cannot meet a one-second full-resolution target for this
  fixture; reaching it requires evaluating a faster or hardware-accelerated
  HEVC decoder such as libheif's FFmpeg decoder.
- 2026-09-02: The first release run of the generated 100k-entry directory E2E
  scenario measured about 18 ms for `open_folder`, but about 2.2 s before the
  first 250-item page returned and about 2.3 s to the first-page paint. The
  300 ms gate is intentionally failing until directory paging avoids this
  synchronous scan.

## RAW Pipeline Findings

nomacs' [current RAW loader](https://github.com/nomacs/nomacs/blob/adb8c789a52205a6ae931af8102b987e2fd359ed/ImageLounge/src/DkCore/DkBasicLoader.cpp)
confirms the main fast path used here: try metadata
and LibRaw embedded previews first, then develop RAW pixels only when the fast
preview is unavailable or explicitly not requested. Its surrounding thumbnail
and image-loader architecture also:

- checks memory and disk thumbnail caches before decoding;
- coalesces duplicate thumbnail requests and supports cancellation;
- prioritizes the current image and preloads the next image within a memory
  budget;
- allows a suitable larger cached thumbnail to satisfy a smaller request.

OxyViewer now follows the embedded-preview fast path and progressive loupe
display. Coalescing, up-tier cache reuse, and scoped priority scheduling have
landed since the original investigation. Remaining improvements are:

1. Add fixture-backed timings for embedded-preview extraction, half-size
   development, resize, JPEG encode, and warm-cache lookup beyond the current
   end-to-end fixture budget test.
2. Add cooperative cancellation checkpoints to the expensive development path;
   current scheduling can reorder pending work but does not preempt native decode.
3. Extend the initial macOS Core Image/ImageIO/LibRaw comparison to additional
   camera vendors and verify color, orientation, detail, and browser paint time.
4. Increase thumbnail concurrency only after measuring memory and storage
   pressure. The current single thumbnail lane protects against many concurrent
   half-size RAW fallbacks.
5. Evaluate RawSpeed/OpenMP only for the measured half-size fallback bottleneck;
   neither improves the normal embedded-preview path.

- 2026-09-06: Compared cache-ready output for the same 42 MiB, 4672x7008 Sony
  ARW over five serial runs on the reference Apple Silicon Mac. Warm medians at
  512/4096/full were 350/1381/1619 ms for ImageIO, 455/704/530 ms for Core
  Image RAW, and 661/4298/15125 ms for LibRaw development. Quick Look measured
  221/4267/8840 ms but produced 275 KiB/52 MiB/148 MiB PNG artifacts, so it was
  removed from production planning because it does not satisfy the JPEG cache
  and transport contract. The measured macOS fallback order is ImageIO -> Core
  Image -> LibRaw for 512, and Core Image -> ImageIO -> LibRaw for 4096/full.
  These figures measure native decode, scale, JPEG/PNG encode, and file output;
  they do not include WebView decode/paint and are not yet a multi-vendor
  fidelity qualification.
- 2026-06-12: Full-resolution HEIF display now has a cancellable tile session
  boundary and keeps the 4096 px preview visible during compatibility decode.
  The portable libheif/libde265 backend does not meet the `<1s` hardware target;
  native adapters must only be enabled after cold-load P95 qualification on
  real GPU runners.
  On the current Windows workstation, `DSC00449.HIF` (4672x7008, 10-bit) took
  2.60 s for the fastest full RGB8 libheif/libde265 run; native decode and copy
  took 3.06 s. This is the compatibility baseline, not an acceptance result.
- 2026-06-12: The Windows WIC HEIF adapter now probes installed codecs and each
  selected file before use, then falls back to libheif with stage-specific
  diagnostics. The reference workstation enumerates a WIC HEIF decoder, but it
  rejects `DSC00449.HIF` while opening the decoder with `0x88982F8B`; that
  fixture therefore remains on the compatibility backend. WIC GPU use cannot
  be verified through its public API and is reported as unknown.
- 2026-06-12: Added the minimal `heif_gpu_probe` FFmpeg experiment for Windows
  HEIF hardware paths. The Sony fixture stores its 7008x4672 primary image as
  six HEVC Rext 10-bit 4:2:2 tiles. Intel UHD 770 QSV genuinely decodes these
  tiles into GPU surfaces and can compose them with `xstack_qsv`. The reusable
  probe measured 0.76 s for decode plus GPU composition and 0.95 s for complete
  BGRA readback, with about 1.46 GB peak RSS. RTX 4090 NVDEC rejects the chroma
  format, while D3D12 creates a device but falls back to software output.
  FFmpeg's software HEVC decoder decoded, composed, converted, and copied the
  full RGBA frame in 0.48 s, making FFmpeg software the fastest measured
  Windows backend for this fixture despite the available sub-second QSV path.
- 2026-06-12: The FFmpeg software experiment is now a production HEIF session
  backend. It reads tile-grid offsets dynamically through `ffprobe`, composes
  and crops the primary image, applies display orientation, emits RGBA8, and
  falls back to libheif if probing or decoding fails. Release packages that
  enable this backend must bundle compatible `ffmpeg` and `ffprobe` binaries.
- 2026-06-15: Unified the preview pipeline across RAW/HEIF/TIFF (ADR 0005).
  Every format now flows through a single `oxy_media::preview` dispatcher and
  a shared two-tier scheduler: the frontend `previewQueue` orders pending
  requests `loupe > visible > nearby > preload` (four-level weights via
  `priorityWeight`) and drops requests that scroll out of view before they
  start; the backend `DecodeGate` (generalized from `HeifDecodeGate`) applies
  the same ordering to waiters but never preempts a running decode. RAW
  thumbnails now participate in priority scheduling for the first time
  (previously three FIFO mutex lanes). Cache format is unified to 8-bit JPEG:
  HEIF full resolution switched from 16-bit PNG (`libheif-1.23-sdr-v1`) to
  8-bit sRGB JPEG with embedded ICC (`heif-sdr-jpeg-v1`, via
  `write_srgb_jpeg`); `write_jpeg_atomically_with_icc` is available to all
  decoders. Up-tier reuse (`larger_cached_preview`) generalizes the former
  HEIF-only path to any backend tag, so a cached 4096 px or full-resolution
  JPEG can satisfy a 512 px request without re-decoding. RAW full
  development stays on its own `RAW_FULL_DECODE_LOCK` lane (not the gate) so
  multi-second develops never block thumbnail decoding. Performance budgets
  for cache hits and cold previews are unchanged in shape; the unified JPEG
  cache is expected to be faster on cache write and equal or faster on hit
  versus the prior PNG path.

## Loupe switching cache policy (2026-09-05)

The loupe immediately reuses decoded full, preview, or thumbnail artifacts,
then upgrades through the existing progressive stages. Empty loupe frames use
an unchanging neutral background; focus regions wait for the selected image's
loaded dimensions. Browser resources use LRU retention, with soft protection
for the selection and two neighbors on each side. The 1,024-entry / 512 MiB
limits remain hard limits, including when protected images exceed the budget.
Filmstrip preview warming uses two cancellable workers, ordered by selection,
navigation direction, nearest neighbors, then the visible strip. A virtualized
thumbnail remount paints an already decoded browser resource even while cold
loading is paused, and revisiting it refreshes its LRU position. Full-resolution
generation remains selection-driven. Native decode concurrency is unchanged.
These are scheduling and presentation changes, not new measured NAS latency
results; cold/warm rapid switching and reverse navigation still need fixture
qualification against the budgets above.

The Windows HEIF loupe thumbnail and canvas are sibling React nodes and must
use distinct layer-prefixed keys. Reusing the asset ID for both leaves orphan
thumbnail nodes after selection changes, displaying a previous photo over the
current one. `scripts/loupe-switch.browser.js` exercises the real Loupe with
nine forward/reverse selections and verifies one thumbnail, one canvas, and
the current source after each switch. This checks reconciliation correctness,
not native decode latency.


## Media cache final local verification (2026-09-08)

The parent agent directly rebuilt the release app and ran each route three times
with runner-owned isolated data/cache after the final fixes. Median / observed
P95 (the maximum of only three samples), in milliseconds:

| Route | First preview | Additional observation |
| --- | --- | --- |
| JPEG cold | 57 / 63 | Original-file resource; no managed copy |
| JPEG warm | 53 / 71 | Original-file/OS cache route, not a managed-cache claim |
| RAW cold | 53 / 61 | `DSC00529.ARW` |
| RAW warm | 41 / 43 | Persisted-artifact warmup and measured cache-hit provenance required |
| HIF cold | 45 / 48 | First tile 332 / 334 |
| HIF warm tiles | 39 / 39 | `cachedArtifact`; first tile 232 / 235, all tiles 537 / 538 |

All six preview gates passed. The 150 ms warm budget is not a claim about full
tile completion. A restored projection now reports this request's validated cache
hit rather than replaying old decoder timing/provenance. An earlier warm-RAW run
failed that provenance gate, exposing the missing restore diagnostic; it was fixed
and all three fresh runs passed rather than weakening the gate.

The macOS batch-attribute 100k cold run recorded 637 / 197 / 195 ms first-page
paint (median 197, observed P95/max 637), with enumeration median 100 ms including
attribute acquisition and final pairing median 12 ms. The 637 ms sample was an
initial enumeration outlier, not discarded. There is no historical baseline
comparison or tail-latency qualification. The owner subsequently made 300 ms a
reference target rather than a strict completion gate for this cache task.

`HeifTileCanvas` starts sessions after one microtask rather than waiting for a
requestAnimationFrame that an occluded WKWebView can suspend. Tests keep all RAF
callbacks undelivered and verify StrictMode starts one session, cancellation,
late-listener cleanup and direct-artifact resource leasing. Native sRGB JPEG
outputs now receive explicit ICC APP2 metadata when ImageIO omits it, with a 64 KiB
in-place shift buffer and no pixel re-encoding; real HIF Full/RAW fixture tests
verify the result. Windows/Linux native behavior remains unqualified because the
owner did not provide runners. Rust protocol budgets cover materialization only,
not buffers retained by Tauri/WebView after handoff; no total-process peak-memory
or cross-platform scrolling-latency claim is made by these samples.


## Resource registry lifecycle qualification — 2026-09-09

The resource-budget follow-up uses 512 production registry entries and encoded
memory `max(1 GiB, RAM / 8)`; this 16 GiB macOS machine reported 2 GiB. Protocol
materialization stays independently capped at four responses / 128 MiB. These
limits do not bound total process/WebView memory. The injected 128 MiB test fills
four 32 MiB resources, rejects another byte, releases/evicts one and successfully
reuses its reservation. File/staged entries consume zero encoded bytes, with
staged disk usage reported separately. No active UI/read lease is forcibly evicted.

Native pressure runs use the production UI, portrait grid density and overscan,
600 distinct JPEG paths in each grid/list fixture, and 80 distinct hardlinked HIF
paths with rapid selection followed by eight required Full image load events.
They perform concurrent protocol reads during cache clear and prune, then wait
11 seconds to cover publication-grace expiry and a UI heartbeat. Reports are in
`tests/perf/.reports/resource-stress-*.run*.json`; stderr is captured alongside.

| Scenario | Actually painted | Peak entries | Settled entries / displayed | Peak encoded bytes |
| --- | --- | --- | --- | --- |
| Dense grid | 600 distinct JPEGs | 70 | 40 / 40 | 0 |
| List | 600 distinct JPEGs | 28 | 19 / 19 | 0 |
| HIF loupe (three runs) | Eight required Full loads per run after 80 selections | 31 / 32 / 32 | 24 / 20 in all runs | 16,660 / 16,660 / 8,330 |

All pressure runs passed with no budget exhaustion. HIF runs finished in
44.4 / 47.6 / 47.7 seconds, including the 11-second settle wait. Settled encoded,
staged and materialization counters were zero in all three runs. The JPEG fixtures
are synthetic and HIF paths share one source image's contents; this exercises
resource identities and lifecycle pressure, not a diverse-format decode corpus
or a scrolling frame-time distribution. Windows/Linux native validation remains
outstanding under the previously agreed environment limitation.


The final release build also passed three runs of each existing preview gate.
Median / observed P95 (maximum of three samples), in milliseconds:

| Route | First preview |
| --- | --- |
| JPEG cold | 46 / 58 |
| JPEG warm | 53 / 57 |
| RAW cold | 54 / 64 |
| RAW warm | 41 / 46 |
| HIF cold | 53 / 57 |
| HIF warm tiles | 40 / 43 |

Warm RAW required persisted-artifact provenance; warm HIF required
`cachedArtifact` and all tiles painted (538 / 540 ms). All six cold 800 ms / warm
150 ms first-preview gates passed. These are local samples, with no historical
baseline regression claim. The test runner closed each owned native instance.

### Windows session tile and full-cache separation (2026-09-10)

Windows FFmpeg sessions publish JPEG tiles directly, including display sharpening.
A matching full cache is now presented directly: Display when sharpening is enabled,
None when disabled. Cache hits do not apply sharpening again.

After tile publication, foreground admission is released and completion is emitted.
A cancellable dwell and independent cache lane admit assembly of the actual display
tiles; Arc references avoid copying their compressed payloads. Compatible JPEG tiles
now use statically linked libjpeg-turbo 3.1.3 coefficient stitching with fixed Huffman
tables. Output plus largest input coefficient arrays and 8 MiB headroom must fit
256 MiB. Unsupported formats use the background pixel fallback (256 MiB canvas);
corrupt/incomplete inputs fail without publication. Neither path reopens HIF for a
second decode. The subsequent cache check never triggers generic full generation.
See [HIF acceleration TODO](tasks/hif-performance-plan.md) and the linked reports.

The earlier isolated pixel-baseline run on DSC01443.HIF painted all six cold tiles
(backend 665 ms), with the background Display full artifact appearing about 45 s later.
After restarting the app, full request → cache hit was 23.6 ms and full request →
image load was 120.6 ms, without a tile session. These are single-run checks, not
p50/p95 budget certification; raw cold/warm reports are linked from the TODO.

After DCT integration, six Release WebView runs (three local HIF files × two rounds)
completed cold tiles, persisted Display JPEG, and reopened via full artifact without
tile decoding. Median DCT encode was 459 ms (444–483 ms), cache commit 27 ms
(25–93 ms), and full request → warm image load 96 ms. Disk cache was observable
1.24–1.46 s after all tiles painted, including the existing one-second dwell.
Native app peak working set was 262–264 MiB (excludes FFmpeg/WebView children).
Three production files passed independent comparison of all 98,224,128 coefficients
per image. Concurrent next-selection p50/p95 and cross-platform fixture matrices
remain follow-up measurements; see the TODO for raw records and validation limits.
