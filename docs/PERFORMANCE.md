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

- 2026-09-05: Filmstrip summary pagination now measures the rendered unloaded
  boundary instead of multiplying an estimated item width. This avoids cumulative
  drift from scrollbar space and fractional CSS sizing, and rechecks on viewport
  resize and page completion. The browser regression in
  `scripts/filmstrip-pagination.browser.js` mounts the real Loupe with 1,473
  synthetic summaries and delayed pages. All 12 orientation/height/scrollbar-space
  cases pass boundary and end jumps; six simulated classic-scrollbar cases
  reproduce the former missed fetch at item 750. Opening does not eagerly fetch
  additional pages. This verifies pagination behavior in Chromium, not native
  media latency or the release-build timing budgets above.

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

- 2026-09-04: Windows release linking now declares the AOM archive at the
  `oxy-media` boundary. The pinned vcpkg libheif port enables AOM but its Rust
  discovery helper omits that transitive static library; keeping the link
  directive scoped to the media crate also avoids copying AOM into every
  downstream Rust staticlib.

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
  `literal:<NAS_FIXTURE_DIR>/DSC00463.HIF`,
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
3. Benchmark the proposed macOS Core Image full-size backend against LibRaw
   before changing the portable full-detail fallback.
4. Increase thumbnail concurrency only after measuring memory and storage
   pressure. The current single thumbnail lane protects against many concurrent
   half-size RAW fallbacks.
5. Evaluate RawSpeed/OpenMP only for the measured half-size fallback bottleneck;
   neither improves the normal embedded-preview path.
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
