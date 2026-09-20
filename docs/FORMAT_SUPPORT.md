# Format Support

| Format | Discovery | Preview | Metadata read | Metadata write |
| --- | --- | --- | --- | --- |
| JPEG/JPG | Implemented | Original file in WebView; generated paths are not required | Unified native EXIF/XMP/IPTC/ICC/MakerNote reader; sidecar-first editable metadata | XMP sidecar by default; optional embedded sync through ExifTool |
| HEIF/HEIC/HIF | Implemented | Embedded preview followed by a direct ImageIO full JPEG on macOS or progressive full-resolution tiles on Windows/Linux; tile sessions also write a source-derived JPEG for subsequent warm loupe loads | Unified native reader; `libheif-rs` item-table XMP extraction; Sony shooting focus location/frame | XMP sidecar by default; optional embedded sync through ExifTool |
| ARW/CR2/CR3/NEF/DNG/RAF/RW2/ORF | Implemented | Bundled LibRaw 0.22.2 embedded preview; macOS then prefers ImageIO at 512 px and Core Image RAW at 4096/full before the alternate Apple backend and LibRaw development; Windows full development tries qualified WIC then LibRaw (optional system extension); Linux uses LibRaw | Unified native EXIF/XMP/IPTC/ICC/MakerNote reader; embedded XMP with adjacent sidecar override | Embedded XMP rating/color/flag read; adjacent XMP sidecar write and override |
| PNG/WebP | Implemented as secondary formats | Original file in WebView | Unified native EXIF/XMP/IPTC/ICC reader | XMP sidecar rating/color/flag |
| TIFF | Implemented as a secondary format | macOS ImageIO-generated JPEG at 512 px for thumbnail/preview and 4096 px for full; Windows/Linux native preview remains unavailable | Unified native EXIF/XMP/IPTC/ICC reader | XMP sidecar rating/color/flag |

RAW support follows bundled LibRaw 0.22.2. The code builds LibRaw from the
pinned source submodule and does not depend on a developer machine's system package.
The full RAW fixture matrix and Windows/Linux packaging validation remain part
of milestone RAW-1. “Implemented” here describes the code path; it does not mean
that every camera/codec combination has passed the release matrix.

All formats write rating/color/flag to adjacent XMP sidecars by default. A sidecar
overrides embedded XMP when both exist. Embedded reads use the in-process
`oxy-metadata-parser` crate. A configured ExifTool can serve as a background
compatibility fallback after a native parse error; installation is prompted
only when the user explicitly synchronizes into JPEG/HEIF/HIF. The desktop then
offers a pinned, checksum-verified managed download or an existing executable
path. `OXY_EXIFTOOL_PATH` and `PATH` remain deployment/development fallbacks.

The parser is an image-only fork of SiftX pinned at the provenance recorded in
`crates/oxy-metadata-parser/UPSTREAM.md`. Its stable OxyViewer facade retains
normalized fields, raw namespace/name/value tags, and diagnostics so new file
formats and MakerNotes do not require frontend or IPC changes.

For supported Sony ARW, JPEG, and HEIF/HIF files, the loupe can show the
shooting focus location from MakerNote tag `FocusLocation` as a green frame
with a center marker. The exact `FocusFrameSize` is used when present. The
button preference persists; `F` toggles it and holding `Alt` temporarily
reverses it. The overlay remains attached to the image while zooming and
panning. Mapping uses the decoded preview's actual dimensions: matching aspect
ratios map directly, a full developed RAW may expand around an in-camera crop, and otherwise a
conservative centered-crop fallback prevents blindly applying capture
coordinates to a differently shaped preview. A dashed frame indicates an
estimated size when only the exact focus center is recorded. This behavior follows
[Sony Imaging Edge Viewer's focus-frame model](https://support.d-imaging.sony.co.jp/app/imagingedge/en/instruction/2_1_viewer_display.php): show the shooting focus frame in green only when supported capture metadata exists.

Sony ARW and HEIF/HIF files carry the burst values used for grid grouping:
MakerNote `ReleaseMode` (`0 = Normal`, `2 = Continuous`, `5 = Exposure
Bracketing`, `6 = White Balance Bracketing`, `8 = DRO Bracketing`) and
`SequenceNumber` (1-based index inside a burst; `0` marks a single shot).
Both survive the HIF container, so ARW and HIF group identically. Runs are
detected in `oxy_domain::group_bursts`: the index must increase by one and the
timestamps stay within 10 seconds, so a counter reset, a long pause or a
bracketing drive mode ends a run. The grid collapses each run to its first
frame with a `×N` badge; clicking the badge expands it in place.

Custom hierarchical tags use the Lightroom-compatible
`lr:hierarchicalSubject` path syntax (`parent|child`) together with leaf values
in `dc:subject`. Adjacent XMP sidecar keywords and hierarchical paths reconcile
into SQLite when an asset is inspected. Embedded image keywords remain read-only:
the UI deduplicates values also present in the sidecar and renders embedded-only
values as muted tags. A pending database-to-sidecar write temporarily blocks the
reverse import so stale XML cannot restore a just-deleted tag. Sidecars remain the
primary write target; explicit embedded sync copies both arrays into supported
JPEG/HEIF/HIF files through ExifTool.

HEIF decoding uses libheif through `libheif-rs`; release packages must include a
working HEVC decoder, which the pinned FFmpeg libraries provide on macOS/Linux
and libde265 provides on Windows. libheif, libde265 and FFmpeg LGPL distribution
and relinking obligations, plus HEVC patent/licensing requirements, must be
checked per release platform. LittleCMS performs ICC conversion and is linked
statically under its MIT license.

Every platform pins libheif 1.23.4, the minimum accepted version because it
contains the September 2026 security fixes, including three high-severity issues;
the Rust test suite rejects an older linked library. macOS and Linux build the
pinned libheif source with `WITH_FFMPEG_DECODER=ON` and link it statically
against the pinned FFmpeg libraries through `pnpm native:prepare`, so HEVC
decoding does not depend on a host codec. Windows takes the same libheif version
from a checked-out vcpkg revision of the curated registry, where HEVC decoding
comes from libde265 instead; it is not linked against FFmpeg.
