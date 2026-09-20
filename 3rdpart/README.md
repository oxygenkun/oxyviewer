# Third-Party Native Components

This directory records the pinned source or packaging definition for native
components shipped with OxyViewer. Source acquisition is kept reproducible, but
each component retains the upstream build system appropriate to its artifact.

| Component | Pinned source | Integration |
| --- | --- | --- |
| LibRaw 0.22.2 | `3rdpart/libraw` Git submodule | Static library linked into OxyViewer |
| libjpeg-turbo 3.1.3 | `3rdpart/libjpeg-turbo` Git submodule | Static library linked into OxyViewer |
| libheif 1.23.4 | URL and SHA-256 in `3rdpart/libheif/source.json`, built with the pinned FFmpeg decoder on macOS/Linux; the vcpkg curated registry at revision `0635f447edcc25f50645afade4e91a229a35fcdc` (libde265 decoder) on Windows | Static native library linked through `libheif-rs` |
| FFmpeg 9.0.1 | URL and SHA-256 in `3rdpart/ffmpeg/source.json` | Standalone `oxy-ffmpeg` and `oxy-ffprobe` sidecars, plus the static prefix that libheif links on macOS/Linux |
| SCRFD-10G KPS, AdaFace IR-101 | URLs, sizes, SHA-256, entry names, and license summaries in `3rdpart/face-models/managed.json` | User-downloaded ONNX files verified and installed in app data, then executed by the first-party analyzer subprocess through the pure-Rust `tract` runtime; no native library is linked |

Build prerequisites and commands belong in [CONTRIBUTING.md](../CONTRIBUTING.md).
For FFmpeg-specific packaging, verification, and redistribution requirements,
see [the FFmpeg packaging guide](../docs/FFMPEG_PACKAGING.md).

macOS and Linux build both components from the pinned sources through
`pnpm native:prepare`: FFmpeg installs a static prefix under `target/native/ffmpeg`
and libheif links that prefix with `WITH_FFMPEG_DECODER=ON`, so HEVC decoding does
not depend on a host codec. Windows keeps the earlier arrangement: libheif comes
from the vcpkg revision that `.github/workflows/ci.yml` checks out, decodes HEVC
through libde265, and is not linked against FFmpeg. CONTRIBUTING.md repeats the
same vcpkg revision for local Windows setup.

Native components must be pinned by version or immutable source revision, built
for every supported target, and recorded in `THIRD_PARTY_NOTICES.md`. They must
not be silently sourced from an untracked developer system installation.

Face model files are downloaded inputs rather than compiled artifacts. They are
not part of `native:prepare` or an application bundle: the People workbench owns
the explicit download flow, reports progress, verifies the pinned SHA-256, and
installs each file atomically.
