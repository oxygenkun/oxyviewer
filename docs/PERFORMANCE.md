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
- 2026-09-02: HEIF loupe zoom now uses the full tile-session dimensions rather
  than the 512 px placeholder's natural size. A displayed 100% therefore maps
  one source pixel to one CSS pixel and 400% is a true four-times pixel zoom.
  HEIF full-resolution display tiles also receive an optional mild luma
  unsharp mask, enabled as the default Standard setting to match Sony Imaging
  Edge Viewer edge definition without modifying source files or caches. macOS
  uses Accelerate/vImage for the full-frame convolution before tile slicing,
  so neighboring samples cross tile boundaries. Three Debug runs on the NAS
  `DSC00518.HIF` fixture measured 36-52 ms ImageIO decode, 42-49 ms sharpened
  tile publication, and 78-101 ms backend total.
- 2026-09-01: The HEIF loupe path now uses the 512 px JPEG only as a temporary
  base layer and starts the full-resolution tile session without requesting a
  second 4096 px JPEG. On macOS, HEIF preview JPEGs are encoded by ImageIO and
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
- 2026-06-13: HEIF decode work is now globally single-flight with three
  priorities: the current loupe image first, visible grid/filmstrip thumbnails
  second, and nearby overscan thumbnails last. Grid and filmstrip HEIF
  thumbnails also use a cancellable priority queue, so requests that have not
  started are discarded or reprioritized as visibility changes instead of
  continuing after navigation.
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
display. Remaining improvements should be adopted in this order:

1. Add fixture-backed timings for embedded-preview extraction, half-size
   development, resize, JPEG encode, and warm-cache lookup beyond the current
   end-to-end fixture budget test.
2. Coalesce in-flight requests by source identity and allow a suitable cached
   larger preview to satisfy a smaller request.
3. Add cancellable priority scheduling for selected-image previews, visible
   thumbnails, and low-priority preloads.
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
  requests `loupe > visible > nearby` (three-level weights via
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
