# Format Support

| Format | Discovery | Preview | Metadata read | Metadata write |
| --- | --- | --- | --- | --- |
| JPEG/JPG | Implemented | Foundation implemented | Planned ExifTool worker | Planned embedded write |
| HEIF/HEIC/HIF | Implemented | macOS Quick Look fallback; planned libheif adapter | Planned ExifTool worker | Planned embedded write |
| ARW/CR2/CR3/NEF/DNG/RAF/RW2/ORF | Implemented | Bundled LibRaw 0.22.1 embedded preview with half-size development fallback; macOS Quick Look final fallback | Planned ExifTool worker | Planned XMP sidecar |
| PNG/WebP/TIFF | Implemented as secondary formats | Foundation/Planned | Read-only planned | Not in MVP |

RAW support follows bundled LibRaw 0.22.1. The code builds LibRaw from the
vendored source and does not depend on a developer machine's system package.
The full format fixture matrix and Windows/Linux packaging validation remain
part of milestone RAW-1.

HEIF codec availability and patent/licensing requirements must be checked per
release platform.
