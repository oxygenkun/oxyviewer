# ADR 0009: Scoped multi-level work scheduling

## Status

Accepted.

## Context

Virtualized grid, list, loupe filmstrip, inspector, and background cache warming
can request the same source-derived artifact at the same time. A task's useful
priority changes as selection and viewport state move, but its cache value does
not disappear merely because one component stops observing it.

The previous preview API mixed four concerns in every request: resource identity,
component lifetime, semantic labels such as `visible`, and a numeric queue order.
Point updates could promote work but could not atomically replace a viewport. A
late update from one consumer could also demote work still needed by another.

Cancellation is not a priority. Removing a pending task, cooperatively stopping
running work, stopping a frontend promise wait, and demoting cacheable work are
different operations and must not share a sentinel priority such as `level -1`.

## Decision

OxyViewer uses a generic scoped intent scheduler in `oxy-runtime`. Business
scenarios configure named tiers, while the scheduler compares the normalized
position `(tier, rank)`: lower tier and lower rank are more important.

Each scheduling intent has:

- a stable task key;
- a `scopeId` identifying its owner, such as `grid-viewport` or
  `loupe-filmstrip`;
- a monotonically increasing scope `epoch`;
- a tier and an ordered rank inside that tier.

The effective position for a task is the best position requested by any scope.
One scope therefore cannot demote a task that another scope still needs at a
higher priority.

The scheduler exposes two mutation forms:

1. `reconcile(scope, epoch, ordered intents, omitted policy)` atomically replaces
   a scope's desired set. An omitted prior intent may be removed or retained at a
   specified lower tier at the front or back. Only intents owned by that scope are
   affected.
2. `upsert(scope, epoch, task, tier, front/back)` changes one intent. It is used
   for selection, newly discovered work, and focused interaction updates.

An epoch older than the accepted epoch for its scope is rejected. Reconciliation
returns only keys whose effective position changed so a concrete queue can
reprioritize them while holding its own lock.

Lifecycle remains separate:

- `demote` keeps cacheable work and changes only its pending order;
- `release` removes one scope's intent;
- `removePending` may discard work only when no remaining consumer needs it;
- `cancelCooperative` is executor-specific and may stop running work only at a
  safe cancellation point.

Preview scheduling is the first instance. Its policy is:

| Tier | Preview meaning |
| ---: | --- |
| 0 | selected loupe image |
| 1 | viewport-visible thumbnails |
| 2 | nearby overscan thumbnails |
| 3 | background/cache preload |

When the selected asset is visible, it is submitted only at tier 0. Other visible
items are ranked around the selection according to the surface policy. When the
selection is outside a viewport, visible items are ranked from the viewport center
with an optional scroll-direction bias. Background ordering alternates nearest
right and left items until one side is exhausted, then appends the longer side.

Frontend viewport reconciliation is bounded to selected, visible, and overscan
items and follows the virtualized viewport snapshot. It does not resend an entire
large directory on every scroll event. Stable background enumeration is updated
when the directory, filter, sort, selection, or loaded page set changes.

Each frontend scope coalesces adjacent full snapshots with latest-wins semantics,
deduplicates identical content, and serializes IPC in epoch order. Viewport scopes
dispatch at most once per 50 ms after animation-frame coalescing. Background scopes
first debounce policy construction for 150 ms, then dispatch at most once per 200 ms.
At most one IPC call per scope is in flight, so a slow native bridge creates
backpressure instead of an unbounded client-side request backlog. Scope release
drops unsent mutations and submits the final empty snapshot as soon as the current
in-flight call permits. Thumbnail components do not emit per-cell promotion IPC;
the viewport scope is the single authority for those priority changes.

## IPC contract

The frontend submits preview schedule snapshots and point updates, not queue
implementation details. Contracts use camelCase and contain no image bytes.

```text
reconcilePreviewSchedule(scopeId, epoch, intents, omittedPolicy)
upsertPreviewSchedule(scopeId, epoch, task, tier, placement)
releasePreviewSchedule(scopeId, epoch, task)
```

`getPreview` still creates or attaches to concrete resource work and returns the
accepted projection. The scheduler determines its effective pending position from
all current scope intents. State-revision and source-revision rules from ADR
0008 remain authoritative.

## Consequences

- A viewport change becomes one atomic scheduling transaction.
- Grid, list, filmstrip, selection, and background preload share one scheduling
  vocabulary without sharing UI-specific ordering code.
- Offscreen work can continue warming the cache without delaying visible work.
- Shared consumers are safe: effective priority is aggregated instead of last
  writer winning.
- Queue implementations need scope/epoch bookkeeping and explicit cleanup after
  work completes or a scope is released.
- Full reconciliation must remain window-bounded; submitting every item on each
  scroll would replace queue contention with IPC and sorting overhead.

## Rejected alternatives

### Encode cancellation as the highest or lowest priority

Cancellation has no valid position in a total order and can accidentally execute
if treated as a queue level. It also cannot express pending-only versus cooperative
running cancellation.

### Let the last component update win

The same artifact may be visible in the filmstrip and background in another
surface. Last-writer priority would allow the background consumer to demote work
still required by the visible one.

### Submit the complete directory on every scroll frame

This is atomic but makes interaction cost proportional to directory size. The
viewport scope remains bounded, while stable background policy changes less often.
