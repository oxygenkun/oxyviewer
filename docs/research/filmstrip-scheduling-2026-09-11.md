# Filmstrip request sharing and bounded retention

Date: 2026-09-11. Implements the bounded scheduling follow-up from
[the investigation](filmstrip-scroll-diagnosis-2026-09-11.md).
The complete sanitized A/B samples are in
[the measurements JSON](filmstrip-scheduling-measurements-2026-09-11.json).

## Behavior

- Components and preloaders share one in-flight thumbnail request for the same
  asset identity and source summary. Each consumer can leave independently.
  A microtask coalesces effect replacement before dispatch or cancellation;
  the pool drops completed results and keeps projections as the result cache.
- Filmstrip schedules include the selected large image, visible thumbnails,
  and overscan. The selected base retains loupe priority; visible/nearby work
  follows the latest viewport and distance from its center. Native thumbnail
  request priorities are fallbacks, so an old consumer's initial visible rank
  cannot prevent the viewport from demoting pending work to nearby.
- At most two worker-active thumbnails without consumers may finish, only in
  the current directory and generation. The actual bound is
  `min(2, worker_count - 1)` and is zero with one worker. Unobserved pending work
  is removed immediately. Rejoining a retained active identity shares its work.
- Directory changes and invalidation cancel retained work. Source revision,
  generation, publication, and resource lease checks remain in effect.
  Preview/full do not receive the thumbnail retention exception.
- An undersized thumbnail may be labeled Interim but has no upgrade phase.
  Its completed request now detaches its abort listener; leaving the viewport
  no longer sends a cancellation for that already-completed request.

The [preview architecture](../architecture/03-preview-pipeline.md) specifies the
scope, consumer, and cancellation contracts. This change does not introduce
whole-directory FIFO reading or alter media backend concurrency.

## Packaged A/B

Both binaries were built with `pnpm tauri build --no-bundle`. The before binary
was saved immediately before this change and contains the same unrelated
workspace changes as the after binary. Every run used a fresh process, app
database, artifact cache, and WebView profile. No normal user state or source
photographs were modified.

The fixture was one local directory containing 241 real HIF files on a Windows
host with 24 logical processors. It fits the first 250-item page. The OS file
cache was not flushed. Before scrolling, the initial selected full image and
its background artifact commit both completed.

A sweep moved to 80% of the filmstrip in 32 updates with requested 16 ms gaps.
Actual movement lasted 619–693 ms. The target had eight visible thumbnails.
Readiness required every displayed image to have `complete` and nonzero
`naturalWidth`, without visible placeholders. Polling requested 25 ms intervals
and was followed by two animation frames. All windows were visible and focused.
IPC instrumentation only observed messages and thumbnail callbacks; it did not
modify admission or cancellation.

Execution order was before-1, after-1, after-2, before-2, before-3, after-3.

| Stop-to-all-visible-ready | Before | After |
| --- | ---: | ---: |
| Pair 1 | 888.5 ms | 634.0 ms |
| Pair 2 | 740.8 ms | 570.4 ms |
| Pair 3 | 691.5 ms | 715.1 ms |
| Median | **740.8 ms** | **634.0 ms** |

Median whole-viewport waiting fell 14.4%; the third pair was 3.4% slower.
Median first-visible readiness fell from 416.1 to 297.4 ms. These small samples
show variability, not a guarantee for each scroll or storage device.

| Per sweep | Before | After |
| --- | ---: | ---: |
| Thumbnail IPC requests | 371–374 | 199 |
| Distinct requested files | 199 | 199 |
| Cancellation messages | 356–359 | 147–153 |
| Rejected thumbnail IPC calls | 253–277 | 116–129 |

The after sweeps each issued exactly one thumbnail request per distinct file.
Median request count fell 46.8% and cancellation messages 57.7%. Rejections
include cancellation and are not counts of failed or interrupted decodes.
Every destination viewport completed. This comparison measures the combined
change; it does not isolate the benefit of retention from sharing or reordering.

## Cold jumps and warm return

A separate fresh process per version jumped to three cold regions and then
returned to the first. These are one sample per region, not repeated medians.

| All visible ready | Before | After |
| --- | ---: | ---: |
| 25% cold | 330.7 ms | 241.7 ms |
| 50% cold | 356.3 ms | 329.7 ms |
| 80% cold | 345.8 ms | 372.1 ms |
| Warm return to 25% | 25.5 ms | 25.9 ms |

Across all four positions, requests fell from 128 to 104 and cancellation
messages from 145 to zero. Both versions had zero rejected thumbnail calls.
The original investigation already established that most cancellations in
these settled-jump runs referred to completed work.

No JS long task above 50 ms was observed in the A/B probes. A few RAF gaps
reached about 106 ms, so this is not a claim of continuous frame-rate compliance
or a hardware presentation-time measurement. Metadata-driven rendering work
identified by the investigation was not changed here. NAS, HDD, and directories
requiring further pages were not benchmarked by this A/B.

## Regression checks

- `pnpm check`, all 184 frontend tests, and `pnpm build` passed.
- Rust formatting, workspace Clippy with warnings denied, and workspace tests
  passed. Focused tests cover shared subscribers, same-turn cancellation,
  source changes, stale result release, viewport demotion, bounded active
  retention, pending removal, directory changes, and invalidation.
- The rebuilt release passed `cold-preview-hif` (88.3 ms versus the 800 ms
  budget), `navigation-cache-hif` (eight returns in 4.7–9.5 ms with memory hits),
  and `filmstrip-scroll-hif` (three stops ready in 25.4–26.9 ms over 80 fixture
  paths). These single-run scenarios do not establish a new regression baseline.
- `resource-stress-small-grid` passed with 600 JPEG paths, 283 peak native
  entries under the 512 limit, and 78 settled entries for 78 displayed images.
  Its existing macOS-only JPEG generator initially skipped on Windows. The
  same 160×120 gradient fixture was generated using System.Drawing, then the
  unchanged scenario was rerun. Cache maintenance and live resource-read
  checks passed.
- `resource-stress-hif` also passed rapid selection across 80 HIF paths,
  full-image returns, and cache maintenance/read checks. Native entries peaked
  at 80 and settled to 32 under the 512 limit.

Reproduce the packaged smoke scenarios with:

```sh
pnpm tauri build --no-bundle
node scripts/perf/perf-e2e.mjs --scenario cold-preview-hif --scenario navigation-cache-hif --scenario filmstrip-scroll-hif --scenario resource-stress-small-grid --runs 1
node scripts/perf/perf-e2e.mjs --scenario resource-stress-hif --runs 1
```

The A/B procedure above can be repeated on any chosen local HIF directory using
separate app data, cache, and WebView profiles for each binary. Raw timing and
IPC traces remain in the ignored performance-report directory; committed
measurements omit photo paths and machine identifiers.
