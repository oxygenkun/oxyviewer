# Format Support

| Format | Discovery | Preview | Metadata read | Metadata write |
| --- | --- | --- | --- | --- |
| JPEG/JPG | Implemented | Foundation implemented | Planned ExifTool worker | Planned embedded write |
| HEIF/HEIC/HIF | Implemented | 512/4096 px preview plus cancellable full-resolution RGBA8 tile session; libheif compatibility backend; native hardware adapters are capability-gated and pending GPU qualification | Planned ExifTool worker | Planned embedded write |
| ARW/CR2/CR3/NEF/DNG/RAF/RW2/ORF | Implemented | Bundled LibRaw 0.22.1 embedded preview with half-size preview fallback, followed by full-resolution loupe development; macOS Quick Look final preview fallback | Planned ExifTool worker | Planned XMP sidecar |
| PNG/WebP/TIFF | Implemented as secondary formats | Foundation/Planned | Read-only planned | Not in MVP |

RAW support follows bundled LibRaw 0.22.1. The code builds LibRaw from the
vendored source and does not depend on a developer machine's system package.
The full format fixture matrix and Windows/Linux packaging validation remain
part of milestone RAW-1.

HEIF decoding uses libheif through `libheif-rs`; release packages must include a
working HEVC decoder such as libde265. libheif and libde265 LGPL distribution
and relinking obligations, plus HEVC patent/licensing requirements, must be
checked per release platform. LittleCMS performs ICC conversion and is linked
statically under its MIT license.
