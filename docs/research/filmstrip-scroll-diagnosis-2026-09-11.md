# Filmstrip cold-region latency and cancellation experiments

Date: 2026-09-11. This is an investigation, not a scheduling change.
Machine-independent measurements are recorded in
[the accompanying JSON](filmstrip-scroll-measurements-2026-09-11.json).

The subsequently approved scheduling change and its separate packaged A/B results
are documented in [the implementation follow-up](filmstrip-scheduling-2026-09-11.md).
The observations below describe the pre-change implementation.

## Findings

Keeping every thumbnail request alive did not consistently improve scrolling.
Serializing existing requests made cold jumps slower. A separate experiment
requesting each file exactly once also strongly favored bounded concurrency.

The original impression of a pause followed by several images appearing together
has more than one contributing stage. Development builds showed substantial delay
between native publication and frontend image loading. Rapid scrolling in the
packaged application also created hundreds of preview requests and cancellations
before the final viewport settled. These experiments did not reproduce a stable,
multi-second SQLite lock as the explanation for an individual cold jump.

## Environment and measurement

- Windows Tauri/WebView2, 24 logical processors, 241 real HIF files in one local
  directory. No source photographs were modified.
- Each policy used its own application process, SQLite database, artifact cache,
  and WebView profile. The operating-system file cache was not flushed. This is
  a local-storage result; it does not establish behavior on a NAS or hard drive.
- The directory fits in the initial 250-item page. Consequently these runs do
  not exercise the separate pagination path for much larger directories.
- The initial selected image reached full quality before measurement. The final
  sweep pair additionally waited for its background full-image cache commit.
- Cold jump targets were 25%, 50%, and 80% of the horizontal scroll extent,
  followed by a warm return to 25%. Each target had 8–9 visible thumbnails.
- A sweep moved from the initial position to 80% in 32 updates, requesting a
  16 ms delay between updates. Actual movement took 608–754 ms. This was scripted
  scrolling on the actual WebView, not an OS wheel/inertia recording.
- Visible completion required the displayed image element to have `complete`
  and a nonzero `naturalWidth`, with no visible placeholder. Sampling requested
  25 ms intervals; two animation frames followed completion. Main-thread work
  can delay observations. These are DOM/image readiness measurements, not
  hardware presentation timestamps.

Final policy measurements used a fresh `pnpm tauri build --no-bundle` application
with embedded production frontend and no temporary Rust timing probes. Only the
experiment window's IPC messages were wrapped. The packaged build used WebView
messages, while the earlier development build used fetch-based IPC. Message
counts verified that the final interception actually ran.

## Cold jumps

Times are milliseconds from changing the scroll position to all visible images
being ready. The current implementation retains its normal parallelism.

| Policy | 25% cold | 50% cold | 80% cold | Warm return |
| --- | ---: | ---: | ---: | ---: |
| Current cancellation and scheduling | 359.7 | 326.5 | 327.7 | 26.1 |
| Suppress thumbnail cancellation, retain concurrency | 350.8 | 356.1 | 388.2 | 25.8 |
| Suppress cancellation, serialize existing thumbnail requests FIFO | 670.8 | 1253.7 | 1722.3 | 25.7 |

The serial variant changes admission to one outstanding thumbnail IPC request;
it does not implement a new queue that deduplicates all pending requests. Its
first visible image took 435.1, 713.5, and 1100.0 ms respectively. The current
policy took 153.1, 144.5, and 273.4 ms.

Cancellation-message count alone is not a count of interrupted decodes. Across
the four current-policy jump/return cases, 128 preview requests completed without
errors despite 145 cancellation messages: many cancellations refer to work that
has already finished. The suppression variant observed and suppressed all 145
messages. This is why a high cancellation count cannot by itself prove that
removing cancellation will help.

## Continuous scrolling

Times below start when the scripted movement finished and end when the final
viewport was fully ready. The second pair reversed policy order. Only the final
pair also explicitly checked completion of the initial full-image cache write.

| Pair | Current policy | Suppress every thumbnail cancellation |
| --- | ---: | ---: |
| 1 | 1109.9 ms | 1022.8 ms |
| 2 | 741.6 ms | 1611.0 ms |
| 3, initial background cache settled | 918.8 ms | 1746.0 ms |

The small improvement in the first pair did not repeat. In the final pair, first
visible completion after stopping was 563.8 ms versus 889.0 ms. Total time from
the beginning of movement to full viewport readiness was 1533.7 versus 2354.0 ms.

The final current-policy sweep issued 374 thumbnail requests, of which 268
returned errors, with 359 cancellation messages. The suppression sweep issued
365 requests, suppressed 350 cancellations, and all requests completed without
errors. These are IPC results, not measurements of how many native decodes were
aborted. Keeping requests alive retains work for regions that have already left
the screen; completion of that work did not make the destination viewport faster.

The existing-request serial FIFO variant did not show any destination thumbnail
within its 15-second observation window. Its queued requests subsequently drained
without errors. Because it retains duplicate requests, this is evidence against
simply serializing the existing IPC stream, not a measurement of an optimized
unique-file scheduler.

## Read each file once, in filename order

This separate throughput experiment avoids the serial FIFO duplicate-request
limitation. Both runs requested the same 241 paths exactly once, with no
cancellation. One used one outstanding request; the other used a bounded pool
of 24. The initially displayed filmstrip was already warm in each isolated run.

| Concurrency | All 241 native Ready results | Errors |
| --- | ---: | ---: |
| 1 | 24,333.6 ms | 0 |
| 24 | 1,766.8 ms | 0 |

With one request at a time, the 121st file became ready at 6559.3 ms and the 191st
at 15,480.2 ms. A strict filename-order policy that refuses to promote a distant
viewport would inherit this wait. These are backend Ready results; they do not
claim that all off-screen images were decoded by the browser.

Reading files in filename order is also different from reading adjacent disk
sectors. This experiment makes no claim about their physical placement or the
best concurrency for other storage devices.

## Where the apparent shared pause occurs

Earlier probes kept release Rust and compared a development frontend, a built
frontend served locally, and a real-library database snapshot. The snapshot
contained 52,132 resource-projection rows; only the copy was used by the probe.

- Development frontend: one cold target completed in 781.0 ms.
- Temporarily holding metadata-store updates: another cold target completed in
  434.8 ms. This intervention supports a frontend contribution, but the targets
  differ, so it does not establish an exact percentage improvement.
- Built frontend with the same instrumented backend: three cold targets completed
  in 402.5, 426.7, and 363.6 ms.
- Development frontend with the copied real library: 780.6, 772.4, and 730.9 ms.

One real-library sample gives a useful native/frontend timeline:

| Event | Time after scroll |
| --- | ---: |
| Native thumbnail extraction begins | 240.8 ms |
| Native extraction finishes | 274.0 ms |
| Ready projection is emitted | 278.5 ms |
| Frontend image `load` mark | 732.7 ms |

The extraction itself took 33.2 ms. The approximately 454 ms between native
publication and the frontend load mark includes event delivery, JS/rendering,
image request initiation, and browser image loading. It must not all be labeled
as React render time. In that cold-region trace, SQLite projection lookup maxed
at 4.38 ms, queue-lock waiting at 124.84 ms, and the media protocol at 1.41 ms.
Those results do not support a multi-second database wait for this sample.

Relevant code paths:

- [Loupe](../../apps/desktop/src/components/Loupe.tsx) uses 12-item overscan and
  renders a `Thumbnail` for each virtual filmstrip item. The filmstrip item is
  not memoized. [FilmstripPreviewPreloader](../../apps/desktop/src/components/FilmstripPreviewPreloader.tsx)
  also requests nearby images.
- [App](../../apps/desktop/src/App.tsx) subscribes to the metadata record map and
  maps loaded assets through `projectAssetMetadata` whenever that map changes.
  [projectAssetMetadata](../../apps/desktop/src/lib/metadataProjection.ts) creates
  a new asset object whenever a projection is present, even when displayed fields
  are unchanged. This expands the effect of per-file metadata updates.
- [generatedPreview](../../apps/desktop/src/lib/api.ts) creates a request ID and
  sends cancellation when its signal aborts. Development StrictMode and shared
  preloading can produce several observers of the same identity. One development
  cold target had 72 requests and 57 cancellation messages for about 32 identities;
  native timing recorded 42 extraction calls, rather than one extraction per IPC.
- [RenderQueue::request](../../apps/desktop/src-tauri/src/jobs/preview.rs)
  coalesces active/pending requests, but also retains a scheduling scope for each
  request ID. Suppressing cancellation leaves that scope's old priority alive.
  Updating the viewport's separate scope does not automatically demote it.
- Queue admission and Ready publication still perform SQLite work under shared
  locks. That remains a contention risk worth reducing, but the measured wait
  must be distinguished from this code-level risk.

There is no filmstrip rule that waits for the whole row before showing an image.
The traces are consistent with requests, shared coordination, and frontend work
delaying progress before several independent image completions arrive close
together. More than one of those costs can dominate, depending on build mode
and how far/fast the user scrolls.

## Follow-up design to validate

The evidence favors testing a bounded retention policy: allow a small number of
already-running thumbnails to finish, deduplicate consumers of the same render
identity, and continuously reorder pending work for the new viewport. Old pending
work should be demoted or dropped, with a small directional prefetch window.
Directory changes, source invalidation, generations, and resource leases must
retain their existing semantics.

Independently, reduce frontend updates caused by metadata records whose displayed
fields have not changed and batch appropriate notifications. Removing metadata
updates entirely was only an experimental control. Neither that control nor the
no-cancellation/serial policies were applied to production code.

## Limits and cleanup

These are small controlled samples, not a storage-device matrix or randomized
performance study. The original report of several seconds after a single stop
was not consistently reproduced: packaged cold jumps were around 0.33–0.39 s,
while rapid current-policy scrolling took 0.74–1.11 s after stopping. Frontend
development timings and the slower experimental policies must not be presented
as ordinary production latency.

The existing performance budget prohibits main-thread scroll tasks over 50 ms.
Some sweep RAF gaps approached 100 ms, but a RAF gap is not proof of one long JS
task. These measurements do not constitute a pass of the full performance suite.

Synchronous stderr tracing, ineffective fetch-only packaged interception,
noncanonical-path failures, and command-length failures were excluded from the
comparison. Raw artifacts stay in the ignored performance-report directory;
this document and the JSON omit private paths and machine identifiers.

Temporary Rust probes were removed and the touched source files were verified
against their pre-investigation hashes. The restored release application built
successfully. Existing unrelated workspace changes were preserved. No production
behavior change, commit, or release was made for this investigation.
All experiment application instances, browser sessions, and the temporary Vite
server were closed after collecting the results.
