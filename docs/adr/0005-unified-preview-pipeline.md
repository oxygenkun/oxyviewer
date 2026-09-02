# ADR 0005: Unified preview pipeline across formats

Date: 2026-06-15

## Status

Accepted. The size-based stage vocabulary and IPC signature in this ADR were
refined by [ADR 0006](0006-semantic-render-level-graph.md).

## Context

Before this change the preview pipeline branched on `AssetKind` in several
places, with two parallel priority vocabularies and two disjoint cache formats:

- The Tauri `get_preview` command held a large `match kind` block dispatching
  to `raw_preview` / `raw_full` / `heif_preview_with_priority` / `heif_full` /
  `system_preview`, each with its own fallback and priority-mapping logic.
- HEIF previews were ordered by a `HeifDecodeGate` with three priorities;
  RAW previews had **no** priority system at all — three plain `Mutex` lanes
  served requests in OS-level FIFO order, so a thumbnail scrolling into view
  could not jump ahead of an off-screen one already queued.
- The frontend `heifThumbnailQueue` only covered HEIF thumbnails; RAW, TIFF,
  loupe, and full-detail requests bypassed the queue entirely.
- The only 16-bit, ICC-embedded cache was the HEIF full-resolution PNG
  (`heif::write_srgb_png`). Everything else was already 8-bit JPEG without ICC.
- "Higher quality satisfies lower quality" reuse existed only for HEIF
  (`larger_heif_preview`).

The user asked for:

1. Fast, non-stuttering previews where on-screen visible thumbnails always
   render first, dynamically reprioritizing as the user scrolls.
2. A loupe ("microscope") mode that can view full-resolution images within
   ~1 s.
3. A unified render order across formats, extensible to future image types.
4. Higher-quality caches able to stand in for lower-quality requests.
5. JPEG as the cache format instead of PNG.

## Decision

Adopt a **two-tier unified pipeline** with these properties:

### Two-tier scheduler

- **Frontend tier (`previewQueue`)**: a single serial `SerialTaskQueue`
  covering every format and every stage. It sorts pending work by a three-level
  weight (`loupe=2 > visible=1 > nearby=0`) and drops requests whose
  `AbortSignal` fired before they start (i.e. requests that scrolled out of
  view). As refined by ADR 0006, React Query keys describe artifact identity;
  a shared pending artifact is promoted in place when it becomes visible or
  enters loupe.
- **Backend tier (`DecodeGate`)**: the former `HeifDecodeGate`, generalized to
  all formats. `acquire_decode(priority)` orders waiters `Foreground >
  Visible > Background` but **never preempts a running decode** — higher
  priority only jumps the queue of not-yet-started work. This matches the
  existing gate contract and avoids the complexity of mid-decode
  cancellation.

Scheduling policy is deliberately "order pending only": a running decode is
never interrupted. Tauri's `invoke` cannot carry an `AbortSignal`, so running
work is abandoned by React Query on the JS side but completes on the Rust
thread; the result is simply ignored.

### Full-resolution stage is format-specific

- **HEIF** keeps its tile-streaming session (`HeifDecodeService`, ADR 0004).
  As amended on 2026-09-01, the progressive `<img>` chain for HEIF stops at
  512; `HeifTileCanvas` overlays full-resolution tiles on top. Removing the
  duplicate 4096 decode prevents it from occupying the serial preview queue
  while visible thumbnails wait.
- **RAW** (and future light-decode formats) use a single full-resolution JPEG
  as the terminal stage: `512 → 4096 → full`. RAW full development stays on
  its own `RAW_FULL_DECODE_LOCK` lane and does **not** pass through the unified
  gate, so a ~23 s develop never blocks thumbnail decoding.

### Unified cache layer

- Every preview stage and format writes an **8-bit JPEG**. `write_jpeg_atomically_with_icc`
  optionally embeds an ICC profile in an APP2 chunk via `image`'s
  `JpegEncoder::set_icc_profile`.
- HEIF full-resolution output switched from 16-bit PNG (`write_srgb_png`,
  retained but unused) to **8-bit sRGB JPEG with embedded ICC**
  (`write_srgb_jpeg`). 16-bit depth is sacrificed; the existing HDR→SDR
  tone-map still runs at decode time, so color management is preserved for
  wide-gamut sources. `HEIF_FULL_CACHE_VERSION` bumped to `heif-sdr-jpeg-v1`
  so old PNG caches are ignored (they are rebuildable).
- **Up-tier reuse** (`larger_cached_preview`) generalizes the former HEIF-only
  `larger_heif_preview` to any backend tag: a 512 px request can be served
  from a cached 4096 px (or full) entry without re-decoding, with the browser
  downscaling via CSS.

### Single dispatcher

`oxy_media::preview(path, cache_dir, level, priority, kind)` is the
only entry point the Tauri layer calls for decodable formats. The per-format
`match` block, priority mapping, and fallback chains now live in one place.
Adding a new format means extending the semantic renderer profiles, not
touching the IPC boundary.

### Shared domain vocabulary

ADR 0006 replaces the original size-named `PreviewStage` with `RenderLevel`
(`thumbnail | preview | full`). `PreviewDiagnostics` remains shared, and
`PreviewResult.renderLevel` reports semantics without encoding pixel size.

## Consequences

- **Positive**: one scheduler, one cache format, one dispatcher, one frontend
  queue. Visible thumbnails of any format now outrank off-screen work on both
  tiers. New formats plug into the render-level policy tables.
- **Positive**: cache reuse works across stages and formats; a developed full
  JPEG can satisfy a thumbnail request instantly.
- **Negative**: 16-bit depth is lost for HEIF full resolution. Accepted as
  imperceptible at loupe viewing distances; the tone-mapped 8-bit JPEG with
  ICC remains color-managed.
- **Negative**: a running low-priority decode is not preempted, so under heavy
  scroll the visible thumbnail must wait for the current decode to finish
  (typically tens to a few hundred ms). This was judged preferable to the
  complexity and risk of cooperative cancellation across the IPC boundary.
- **Note**: RAW full development intentionally stays off the unified gate to
  avoid blocking the thumbnail/loupe stages.
