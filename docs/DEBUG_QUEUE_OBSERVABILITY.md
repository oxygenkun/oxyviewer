# Queue observability in debug builds

OxyViewer exposes a read-only scheduler snapshot in debug builds so queue
behavior can be inspected without enabling continuous tracing in normal app
usage.

## Opening the dashboard

Run `pnpm tauri dev`, then use the `DEBUG QUEUES` control in the lower-right
corner. `Ctrl+Shift+D` opens or focuses the same child window. The dashboard
runs in a dedicated WebView window, so the main application remains fully
interactive while folder changes, scrolling, selection, and preview requests
continue to exercise the live queues.

The dashboard samples at 4 Hz and can be paused to inspect a stable snapshot.
It shows WebView preview requests plus the native preview, metadata, lazy
directory-tree, and library-index schedulers. Each scheduler separates pending
and running work and reports its concurrency, semantic priority, stage, asset
path, rank, and number of coalesced consumers. A running library-index row also
reports its root, current directory, pending directory count, and cumulative
asset and directory counts. WebView rows show the concrete artifact level
(`original`, `thumbnail`, `preview`, or `full`) and the requested URL or path.
Multiple DOM consumers of the same URL are collapsed into one row.

## IPC contract

The `get_debug_queue_snapshot` command returns:

```text
DebugQueueSnapshot
  capturedAtUnixMs
  workerWaitMicros
  collectionMicros
  staleQueues[]
  queues[]
    name
    concurrency
    pending[] / active[]
      key, path?, rootPath?, stage, priority, rank?, consumers
      pendingCount?, assetCount?, directoryCount?
```

The shared serialized types live in `oxy-domain`; matching TypeScript types
live in `apps/desktop/src/types.ts`. Queue implementations expose snapshot
methods while holding their existing locks, and `CoalescingPriorityQueue`
visits only logical pending entries rather than stale heap nodes.

## Safety and performance

- The endpoint is available in debug builds; release builds require an explicit
  performance scenario.
- Sampling starts only while the dashboard is open and stops when it closes.
- The endpoint is read-only; it cannot cancel or reprioritize work.
- Every queue and active-request snapshot uses `try_lock`. A busy queue returns
  its last sample and appears in `staleQueues`; it never waits for the business
  lock or reports a never-observed queue as empty.
- Collection runs on a blocking worker and never performs filesystem, database,
  metadata, or decode work. Worker wait and native collection time are reported
  separately from the WebView-observed IPC round trip.
- The dashboard is lazy-loaded and its JavaScript and CSS are split from the
  normal application bundle.
