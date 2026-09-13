# Third-Party Native Components

This directory records the pinned source or packaging definition for native
components shipped with OxyViewer. Source acquisition is kept reproducible, but
each component retains the upstream build system appropriate to its artifact.

| Component | Pinned source | Integration |
| --- | --- | --- |
| LibRaw 0.22.2 | `3rdpart/libraw` Git submodule | Static library linked into OxyViewer |
| libjpeg-turbo 3.1.3 | `3rdpart/libjpeg-turbo` Git submodule | Static library linked into OxyViewer |
| libheif | Cargo's pinned `libheif-sys` source on macOS/Linux; the `3rdpart/vcpkg-ports/libheif` 1.23.3 overlay on Windows | Static native library linked through `libheif-rs` |
| FFmpeg 8.0.1 | URL and SHA-256 in `3rdpart/ffmpeg/source.json` | Standalone `oxy-ffmpeg` and `oxy-ffprobe` sidecars |

Build prerequisites and commands belong in [CONTRIBUTING.md](../CONTRIBUTING.md).
For FFmpeg-specific packaging, verification, and redistribution requirements,
see [the FFmpeg packaging guide](../docs/FFMPEG_PACKAGING.md).

Native components must be pinned by version or immutable source revision, built
for every supported target, and recorded in `THIRD_PARTY_NOTICES.md`. They must
not be silently sourced from an untracked developer system installation.
