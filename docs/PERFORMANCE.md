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
- The generated 100k-entry directory benchmark is not yet recorded. Phase 1
  must add it before its performance gate can be marked complete.

## RAW Pipeline Findings

digiKam's current pipeline confirms the main fast path used here: try metadata
and LibRaw embedded previews first, then fall back to half-size RAW development.
Its surrounding image I/O architecture adds optimizations that OxyViewer should
adopt in this order:

1. Add fixture-backed timings for embedded-preview extraction, half-size
   development, resize, JPEG encode, and warm-cache lookup.
2. Coalesce in-flight requests by source identity and allow a suitable cached
   larger preview to satisfy a smaller request.
3. Add cancellable priority scheduling for selected-image previews, visible
   thumbnails, and low-priority preloads.
4. Increase thumbnail concurrency only after measuring memory and storage
   pressure. The current single thumbnail lane protects against many concurrent
   half-size RAW fallbacks.
5. Evaluate RawSpeed/OpenMP only for the measured half-size fallback bottleneck;
   neither improves the normal embedded-preview path.
