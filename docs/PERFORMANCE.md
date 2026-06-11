# Performance Budgets

Reference budgets are measured on a local SSD with a release build.

| Interaction | Target |
| --- | --- |
| First page from a 100k-file directory | Begin rendering within 300 ms |
| Warm cached loupe preview | Under 150 ms |
| Cold selected-image preview | Under 800 ms |
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
