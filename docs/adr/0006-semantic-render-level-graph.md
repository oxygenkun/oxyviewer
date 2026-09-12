# ADR 0006: Semantic render-level graph

Date: 2026-09-03

## Status

Accepted. This refines the stage vocabulary and dispatcher boundary introduced
by ADR 0005.

Implemented refinement (2026-09-12; Windows measurements and remaining budget gaps documented):
[source-representation planning](../architecture/08-representation-planning.md)
separates JPEG thumbnail delivery from full-quality primary-image delivery,
while preserving the semantic levels below. Its implementation and scrolling
qualification are tracked in the [task plan](../tasks/jpg-quality-and-scroll-performance.md).

## Context

The progressive UI used arrays containing concrete sizes such as `512` and
`4096`. Array position and pixel value also decided whether an image was a
thumbnail, a loupe placeholder, or full detail. Replacing Sony HIF's 512 px
decode with its much faster embedded 160x120 JPEG therefore changed the
interaction graph accidentally: the loupe no longer retained a base image
while full tiles loaded.

Pixel dimensions are an implementation choice of a decoder. They are not a
stable interaction contract. The same artifact may legitimately fulfill more
than one user-visible quality level, and the best mapping may differ by file
type and operating system.

## Decision

Use three semantic render levels shared by Rust and TypeScript:

| Level | Interaction guarantee |
| --- | --- |
| `thumbnail` | Fast representation for grid, list, and filmstrip scrolling |
| `preview` | Optional fit-to-window artifact for callers that explicitly request it |
| `full` | Best available pixel-inspection representation |

The interaction graph is fixed independently of formats:

```text
thumbnail surface: thumbnail
loupe surface:     thumbnail -> full
```

The frontend render profile maps every `(platform, asset kind, level)` to a
renderer class: original image, generated image, HEIF tile session, or reuse of
another level. `Thumbnail` consumes this plan and never infers behavior from
width, height, a numeric request size, or an array index.

The IPC command accepts `level`, not `mode + maxSize`. `oxy-media` owns the
concrete native method and target size:

| Windows mapping | `thumbnail` | `preview` | `full` |
| --- | --- | --- | --- |
| JPEG/PNG/WebP | original | original | original |
| RAW | LibRaw 512 | LibRaw 4096 | embedded/full development |
| HIF/HEIF | embedded/decoded 512 | optional decoded 4096 | HEIF full/tile session |
| TIFF | system 512 | system 512 | system 4096 |

Other platforms can provide a different profile without changing the
interaction graph. The Sony HIF fast path may satisfy the thumbnail with its
embedded 160 artifact; an explicit `preview` request may use that artifact as
an interim result before upgrading. A platform should diverge only after its
native path has fixture-backed performance and fidelity evidence.

React Query keys identify the resolved artifact method, not scheduling
priority. Consequently HIF grid and loupe observers share the same
`generatedImage:thumbnail` cache entry. If the shared request is still pending,
the loupe raises that task's queue priority in place; it does not create a
second decode. Running work remains non-preemptive.

`PreviewResult.renderLevel` reports semantics. It deliberately does not encode
pixel dimensions in an enum name.

## Consequences

- Loupe reuses the already requested thumbnail below its full-detail renderer;
  it does not require a separate `preview` decode.
- Sony HIF keeps its thumbnail painted below the tile canvas and performs no
  redundant fit-to-window HEVC preview decode.
- Cache identity, render progression, and queue priority are separate concerns.
- Adding a format requires a frontend renderer profile and a backend native
  method mapping, both expressed against the same three levels.
- Old `PreviewMode`, `PreviewStage`, `thumb512`, `loupe4096`, and IPC `maxSize`
  contracts are removed rather than kept as aliases that could reintroduce
  size-dependent behavior.
