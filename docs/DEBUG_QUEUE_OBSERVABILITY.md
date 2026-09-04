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
It shows WebView preview requests plus the native preview, metadata, and lazy
directory-tree schedulers. Each scheduler separates pending and running work
and reports its concurrency, semantic priority, stage, asset path, rank, and
number of coalesced consumers. WebView rows also show the concrete artifact
level (`original`, `thumbnail`, `preview`, or `full`) and the requested URL or
path. Multiple DOM consumers of the same URL are collapsed into one row.

## IPC contract

The `get_debug_queue_snapshot` command returns:

```text
DebugQueueSnapshot
  capturedAtUnixMs
  queues[]
    name
    concurrency
    pending[] / active[]
      key, path?, stage, priority, rank?, consumers
```

The shared serialized types live in `oxy-domain`; matching TypeScript types
live in `apps/desktop/src/types.ts`. Queue implementations expose snapshot
methods while holding their existing locks, and `CoalescingPriorityQueue`
visits only logical pending entries rather than stale heap nodes.

## Safety and performance

- The endpoint rejects calls from release builds.
- Sampling starts only while the dashboard is open and stops when it closes.
- The endpoint is read-only; it cannot cancel or reprioritize work.
- Snapshots briefly take existing queue locks and never perform filesystem,
  database, metadata, or decode work.
- The dashboard is lazy-loaded and its JavaScript and CSS are split from the
  normal application bundle.
