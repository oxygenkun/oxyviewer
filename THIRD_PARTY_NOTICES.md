# Third-Party Notices

This repository is structured to preserve the option of proprietary
distribution. Before shipping installers, record the exact versions and
licenses of every bundled dependency here.

Native runtime components:

| Component | Purpose | Intended integration |
| --- | --- | --- |
| LibRaw 0.22.1 | RAW preview extraction and decoding | Vendored unmodified source, statically linked under the CDDL-1.0 option; release legal review pending |
| OxyViewer metadata parser (SiftX fork at `fcf8e3d`) | Pure-Rust EXIF/XMP/IPTC/ICC/MakerNote reads | Modified image-only fork, statically linked under the MIT OR Apache-2.0 license; upstream notices retained |
| libheif | HEIF/HEIC decoding | Dynamically linked, with codec licenses reviewed per platform |
| ExifTool | Optional metadata compatibility/write worker | Separate process; Artistic/GPL terms reviewed before release |

OxyViewer does not link Exiv2 because the product must retain the option of
closed-source commercial distribution.
