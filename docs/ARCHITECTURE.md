# Architecture

OxyViewer uses a thin Tauri command layer over independent Rust crates. The
frontend never receives image bytes through JSON IPC.

## Runtime Flow

1. The user selects a folder through the native dialog.
2. `oxy-fs` opens a session and scans only the requested page of immediate
   children. File names and basic stat data are returned first.
3. The React workspace renders summaries and requests expensive work only for
   visible or selected assets.
4. Future `oxy-runtime` workers prioritize loupe previews, visible thumbnails,
   selected metadata, directory background work, then library indexing.
5. `oxy-library` persists rebuildable cache and explicit library roots in
   SQLite WAL mode.

RAW assets use bundled LibRaw 0.22.1. The media pipeline extracts an embedded
preview first and falls back to half-size RAW development when necessary.
Generated JPEGs are stored in the rebuildable app cache, with separate cache
entries and bounded decode lanes for grid thumbnails and loupe previews. The
loupe then develops a full-resolution RAW in a separate decode lane and upgrades
the displayed preview only after the full-resolution JPEG is ready. Full RAW
development preserves fine detail by avoiding full-strength pre-demosaic noise
reduction and applies modest output sharpening for loupe viewing. macOS Quick
Look remains a final compatibility fallback for preview generation.

HEIF/HIF assets use a progressive pipeline similar to RAW: a 512 px and 4096 px
Quick Look preview is shown first on macOS, then the selected image is upgraded
in a dedicated decode lane. The full-detail path decodes the primary image with
libheif, preserves 9-16 bit samples during color conversion, maps HLG/PQ content
to SDR, and writes a color-profiled 16-bit PNG cache entry. Quick Look at 8192 px
remains the compatibility fallback when the packaged HEVC decoder rejects a
stream. TIFF assets still use the operating system preview generator.

## Boundaries

- `oxy-domain`: serialized public contracts shared by every layer.
- `oxy-fs`: path identity, supported-file discovery, sidecar pairing, and safe
  file operations.
- `oxy-media`: preview and thumbnail contracts. Native RAW/HEIF adapters live
  behind this boundary.
- `oxy-metadata`: XMP policy and the persistent ExifTool process boundary.
- `oxy-library`: rebuildable SQLite cache and explicit-root search.
- `oxy-runtime`: priority and cancellation vocabulary.
- `src-tauri`: state ownership, Tauri commands, and event publication only.

See `docs/adr/0001-no-blocking-import.md` for the core product decision.

## Current Phase 1 Behavior

The current directory scanner returns immediate-child summaries through a
paged IPC contract and the frontend virtualizes loaded pages. It deliberately
does not recurse or index a folder during open. Filesystem watcher events,
background thumbnail scheduling, and measured 100k-directory tuning are the
next Phase 1/2 steps.

Visited directories have a rebuildable, in-memory snapshot cache in `oxy-fs`.
Asset pagination, sorting, and filtering reuse that non-recursive snapshot
until the user explicitly refreshes the current folder.
