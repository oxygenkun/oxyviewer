# Performance Budgets

Reference budgets are measured on a local SSD with a release build.

| Interaction | Target |
| --- | --- |
| First page from a 100k-file directory | Begin rendering within 300 ms |
| Warm cached loupe preview | Under 150 ms |
| Cold selected-image preview | Under 800 ms |
| Full-resolution RAW development | Measured separately; preview remains visible |
| Search/filter response over loaded page | Under one animation frame |
| Main-thread scroll work | No long task above 50 ms |

## Verification Log

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
- The generated 100k-entry directory benchmark is not yet recorded. Phase 1
  must add it before its performance gate can be marked complete.

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
