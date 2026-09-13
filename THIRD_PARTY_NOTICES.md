# Third-Party Notices

This repository is structured to preserve the option of proprietary
distribution. Before shipping installers, record the exact versions and
licenses of every bundled dependency here.

Native runtime components:

| Component | Purpose | Intended integration |
| --- | --- | --- |
| FFmpeg 8.0.1 | HEIF probing, HEVC decoding and JPEG/BMP tile output | Unmodified pinned source, standalone `oxy-ffmpeg` / `oxy-ffprobe`, LGPL-2.1-or-later build with GPL/nonfree/version3 disabled; license and build recipe bundled under `licenses/ffmpeg`, corresponding source attached to each GitHub Release |
| LibRaw 0.22.2 | RAW preview extraction and decoding | Pinned upstream Git submodule, statically linked under the CDDL-1.0 option; release legal review pending |
| OxyViewer metadata parser (SiftX fork at `fcf8e3d`) | Pure-Rust EXIF/XMP/IPTC/ICC/MakerNote reads | Modified image-only fork, statically linked under the MIT OR Apache-2.0 license; upstream notices retained |
| libheif | HEIF/HEIC decoding | Statically linked through the pinned Cargo source on macOS/Linux and the pinned vcpkg overlay on Windows; codec licenses reviewed per platform |
| libjpeg-turbo 3.1.3 | JPEG thumbnail decoding and tile coefficient stitching | Pinned upstream Git submodule, statically linked with SIMD; IJG/BSD-3-Clause/zlib notices retained in `3rdpart/libjpeg-turbo/LICENSE.md` and `README.ijg` |
| ExifTool | Optional metadata compatibility/write worker | Separate process; Artistic/GPL terms reviewed before release |

OxyViewer does not link Exiv2 because the product must retain the option of
closed-source commercial distribution.

This software uses FFmpeg under the GNU Lesser General Public License version
2.1 or later. Build instructions and the pinned source manifest accompany the
application in its `licenses/ffmpeg` resources. The exact corresponding FFmpeg
source archive is a separate asset on the matching GitHub Release. See
[`docs/FFMPEG_PACKAGING.md`](docs/FFMPEG_PACKAGING.md) for the pinned source and
distribution procedure. FFmpeg libraries are linked only into the standalone
FFmpeg programs, not into the OxyViewer application.

This software is based in part on the work of the Independent JPEG Group.
libjpeg-turbo copyright and license texts are retained in
[`LICENSE.md`](3rdpart/libjpeg-turbo/LICENSE.md) and
[`README.ijg`](3rdpart/libjpeg-turbo/README.ijg); include these notices when
distributing binaries that contain the static JPEG backend.
