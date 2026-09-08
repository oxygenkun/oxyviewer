# ADR 0004: HEIF Full-resolution Decode Sessions

## Status

Accepted

## Decision

Full-resolution HEIF display is selected by the Rust-owned `start_heif_full`
boundary. It returns either a completed artifact projection or session metadata.
For sessions, pixel data is exposed as explicit RGBA or JPEG tile payloads
through `oxy-media://` and announced with `heif-tile-ready` events.

Backend order is platform-specific and capability-probed, with FFmpeg and the
portable libheif/libde265 backend as fallbacks. Native APIs are not reported as
hardware accelerated unless that fact can be verified.

Only one selected-image full-resolution HEIF session is active. Starting a new
session cancels the previous generation and clears its in-memory tiles. The
semantic preview remains visible beneath the Canvas while full-resolution tiles
arrive. The canonical full JPEG is always unsharpened; optional display
sharpening uses the tile path and never changes cache identity or pixels.

## Current Implementation

The session, cancellation, diagnostics, protocol, Canvas, compatibility
backend, Windows WIC adapter, macOS ImageIO adapter, and FFmpeg software
tile-grid adapter are implemented. WIC and ImageIO are selected only when the
installed native decoder accepts the selected file. FFmpeg dynamically reads
tile offsets from `ffprobe`, decodes and composes the primary grid, and falls
back to libheif on failure. WIC and ImageIO acceleration are reported as
unknown because their public APIs do not expose reliable GPU-use diagnostics.
The former user-facing hardware toggle and speculative backend states were
removed; future hardware adapters must pass correctness and cold-load P95 tests
on real GPU runners before being reported as hardware accelerated.

## Consequences

- Image bytes never cross JSON IPC.
- Late tiles cannot replace a newer selection.
- Non-grid HEIF publishes tiles after full-frame decode.
- Grid-aware early tile decode remains a platform-adapter optimization.
