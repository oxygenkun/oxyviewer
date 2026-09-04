# ADR 0008: Rust-owned versioned resource projections

## Status

Accepted.

## Context

An asset is currently represented by several independently cached query results:
cheap directory summaries, metadata-enriched summaries, inspector details, thumbnail
and loupe render queries, and HEIF decode-session events. The same rating, label, or
render artifact can therefore exist in several React Query entries. A fast selected
read may update the inspector while the grid waits for an older page-wide request;
the older request can then publish last even though its information was valid first.

The same problem applies to images. A thumbnail, preview, full render, and HEIF tile
are progressively stronger projections of one source asset. Query completion order
is not resource validity order, and frontend component lifetime is not a safe place
to own decoding, deduplication, or cache truth.

Opening a directory must remain cheap and non-recursive. Moving authority to Rust
must not make initial listing wait for metadata parsing, image decoding, or cache IO.

## Decision

Rust owns one versioned resource state per canonical asset path. Metadata and image
artifacts are typed projections within that state. React keeps only a normalized,
read-only mirror needed to render without an IPC round trip.

Every projection update carries:

- a `sourceRevision` describing the source bytes and relevant sidecars;
- a monotonically increasing `validAt` assigned when the read is requested;
- a monotonically increasing `projectionRevision` assigned only when Rust accepts
  the result;
- the exact field or render-level coverage of the result;
- a load state and optional error.

Workers never write frontend state directly. They submit observations to the Rust
coordinator. The coordinator merges covered fields only. A result whose `validAt`
is older than the accepted value for that field is discarded even if it completes
later. Missing values from a completed field read are authoritative; fields outside
the result's declared coverage are preserved.

`sourceRevision` is not a wall-clock timestamp. It includes cheap filesystem
identity first (source size and modification time plus sidecar size and modification
time). Formats that permit in-place metadata writes while preserving those values
must add a targeted metadata-item fingerprint. An explicit refresh invalidates the
current generation even when the cheap identity is unchanged.

All resource work enters a Rust-owned priority queue. Requests for the same path,
source revision, and coverage coalesce. A later consumer may raise an existing
request's priority. The order is:

1. selected metadata or selected full image;
2. loupe preview;
3. visible grid/filmstrip resources;
4. nearby overscan;
5. filter-required metadata;
6. directory/background preload and library indexing.

Metadata detail reads may satisfy summary fields. Higher image render levels may
satisfy lower semantic levels only when the format policy explicitly declares the
artifact suitable; pixel dimensions alone do not imply that relationship.

Rust publishes accepted changes as projection events. Events contain the canonical
path, source revision, projection revision, changed coverage, and the small display
payload. Frontend mirrors accept only increasing projection revisions. Grid, loupe,
inspector, filters, and status UI derive from that mirror and do not maintain
independent authoritative copies.

## IPC and transfer policy

Directory listing returns cheap `AssetSummary` values immediately. It may include a
small already-cached projection, but it never starts synchronous parsing or decode.
The frontend then sends priority hints for selected, visible, nearby, and background
assets.

After restart, metadata loading uses stale-while-revalidate publication: a persisted
ready projection whose source size and modification time still match is emitted
per asset immediately, without waiting for the rest of the page. Rust then validates
the complete revision, including sidecar or embedded-XMP fingerprints. If that
revision changed, the old values may remain visible only during `loading`; the
accepted `ready` observation replaces them for every surface together.

Metadata events carry only normalized fields needed by their requested coverage.
Inspector-only capture data and focus information are requested on selection. Raw
tag dumps and image bytes are not sent through ordinary JSON IPC.

Image projection events carry cache/protocol paths and render diagnostics, not image
bytes. WebView fetch and browser decode remain frontend presentation concerns. The
frontend may preload a returned URL, but it does not decide whether the artifact is
current or which worker result wins.

Filtering is evaluated against Rust's authoritative metadata projection. While
missing fields are queued, matching rows may be published incrementally. A filter
must never treat an unqueried field as an authoritative empty value.

## Process and persistence model

The application process owns the live coordinator. Rebuildable projections that
must survive restart are persisted in SQLite with their source revision and a
transactionally assigned projection revision. Multiple application processes use
SQLite WAL transactions for acceptance ordering; process-local counters alone are
not used to compare cross-process results.

## Consequences

- Selecting an asset publishes metadata once and updates every view together.
- Slow page scans and decodes cannot overwrite later selected reads.
- Metadata and image loading share priority, cancellation, and observability rules.
- React Query remains useful for directory/page transport, but not as the owner of
  per-asset metadata or image validity.
- The frontend needs a small normalized event mirror and reconnection snapshot.
- Rust state and queue code become more involved, and event ordering, stale-result
  rejection, field coverage, and priority promotion require focused tests.
- Migration is incremental: metadata projections move first; preview requests then
  move behind the same coordinator while preserving existing format-specific
  decoders and semantic render policies.

## Implementation state

Metadata summary/detail reads and generated image previews enter Rust-owned
coalescing priority queues; pending and already-running requests share consumers
instead of launching duplicate work. Accepted metadata and image projections are
persisted in the rebuildable SQLite cache before publication to read-only frontend
mirrors. Both observation and publication revisions come from one WAL-serialized
SQLite sequence, and an acceptance transaction returns the newer stored state
instead of allowing an older worker result to overwrite it. Directory and preview
cache invalidation write a revision tombstone, so work that was already running
cannot restore an invalidated result.

HIF metadata revisions include a digest of embedded XMP so an in-place metadata
edit cannot be hidden by unchanged file size or timestamps. Image projections
persist artifact paths and diagnostics only; pixel content remains in the preview
cache or media protocol.

HEIF full-resolution tile sessions already keep their decode state, cancellation,
and publication in Rust and remain a specialized image transport under this
model. Their pixel payload continues to use the media protocol rather than JSON.

The current desktop runtime still executes workers in one native application
process, but acceptance is safe across separate database connections and future
external workers. Cross-process event fan-out is not implemented: another running
application instance observes a committed projection on its next request rather
than receiving the originating process's Tauri event.

## Rejected alternatives

### Patch every React Query cache after a selected read

This improves one symptom but keeps several writable copies and cannot prevent a
late request from restoring stale data.

### Make React/Zustand the authoritative coordinator

This loses state when a component or WebView reloads, duplicates native cache
knowledge, and cannot safely coordinate multiple native workers or processes.

### Compare task completion timestamps

Completion order says nothing about the source version a worker observed. A slow
old task would still be allowed to overwrite a fast new task.
