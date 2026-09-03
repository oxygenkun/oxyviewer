# Format Support

| Format | Discovery | Preview | Metadata read | Metadata write |
| --- | --- | --- | --- | --- |
| JPEG/JPG | Implemented | Foundation implemented | Sidecar-first XMP rating/color plus Sony shooting focus location/frame | XMP sidecar by default; optional embedded sync through ExifTool |
| HEIF/HEIC/HIF | Implemented | 512/4096 px preview plus cancellable full-resolution RGBA8 tile session; Windows WIC and macOS ImageIO native decoders, dynamic FFmpeg software tile-grid backend, and libheif fallback; native hardware use remains pending GPU qualification | Sidecar-first XMP rating/color plus Sony shooting focus location/frame | XMP sidecar by default; optional embedded sync through ExifTool |
| ARW/CR2/CR3/NEF/DNG/RAF/RW2/ORF | Implemented | Bundled LibRaw 0.22.2 embedded preview with half-size preview fallback, followed by full-resolution loupe development; macOS Quick Look final preview fallback | Adjacent XMP rating/color plus Sony shooting focus location/frame | Adjacent XMP sidecar rating/color |
| PNG/WebP/TIFF | Implemented as secondary formats | Foundation/Planned | Read-only planned | Not in MVP |

RAW support follows bundled LibRaw 0.22.2. The code builds LibRaw from the
vendored source and does not depend on a developer machine's system package.
The full format fixture matrix and Windows/Linux packaging validation remain
part of milestone RAW-1.

All formats write rating/color to adjacent XMP sidecars by default. A sidecar
overrides embedded XMP when both exist. ExifTool is optional and is prompted for
only when the user explicitly synchronizes into JPEG/HEIF/HIF; the desktop then
offers a pinned, checksum-verified managed download or an existing executable
path. `OXY_EXIFTOOL_PATH` and `PATH` remain deployment/development fallbacks.

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

HEIF decoding uses libheif through `libheif-rs`; release packages must include a
working HEVC decoder such as libde265. libheif and libde265 LGPL distribution
and relinking obligations, plus HEVC patent/licensing requirements, must be
checked per release platform. LittleCMS performs ICC conversion and is linked
statically under its MIT license.
