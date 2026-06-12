# ADR 0004: HEIF Full-resolution Decode Sessions

## Status

Accepted

## Decision

Full-resolution HEIF display uses a session service owned by `oxy-media`.
Tauri commands return serialized session metadata only. Pixel data is exposed
as tightly packed RGBA8 tiles through the `oxy-media://` protocol and announced
with `heif-tile-ready` events.

The backend order is platform-native hardware decode, platform-native software
decode, then the portable libheif/libde265 compatibility backend. A backend is
never reported as available until its runtime capability probe succeeds.

Only one selected-image full-resolution HEIF session is active. Starting a new
session cancels the previous generation and clears its in-memory tiles. The
4096 px preview remains visible beneath the Canvas while full-resolution tiles
arrive.

## Current Implementation

The session, cancellation, diagnostics, protocol, Canvas, and compatibility
backend are implemented. Windows WIC/Media Foundation, macOS ImageIO/CoreImage,
and Linux FFmpeg/VAAPI adapters currently report unavailable and therefore
fall back to libheif. They must pass correctness and cold-load P95 tests on real
GPU runners before being marked available.

## Consequences

- Image bytes never cross JSON IPC.
- Late tiles cannot replace a newer selection.
- Non-grid HEIF publishes tiles after full-frame decode.
- Grid-aware early tile decode remains a platform-adapter optimization.
