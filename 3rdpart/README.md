# Third-Party Native Components

This directory records the pinned source or packaging definition for native
components shipped with OxyViewer. Source acquisition is kept reproducible, but
each component retains the upstream build system appropriate to its artifact.

| Component | Pinned source | Build entry | Shipped artifact |
| --- | --- | --- | --- |
| LibRaw 0.22.2 | `3rdpart/libraw` Git submodule | `crates/oxy-media/build.rs` compiles the required C++ sources with `cc` | Static library linked into OxyViewer |
| libjpeg-turbo 3.1.3 | `3rdpart/libjpeg-turbo` Git submodule | `crates/oxy-media/build.rs` invokes the upstream CMake `jpeg-static` target and builds the OxyViewer C adapters | Static library linked into OxyViewer |
| libheif | Cargo's pinned `libheif-sys` source on macOS/Linux; the `3rdpart/vcpkg-ports/libheif` 1.23.3 overlay on Windows | Cargo/CMake on macOS and Linux; vcpkg before Cargo on Windows MSVC | Static native library linked through `libheif-rs` |
| FFmpeg 8.0.1 | URL and SHA-256 in `3rdpart/ffmpeg/source.json` | `3rdpart/ffmpeg/prepare.mjs` orchestrates the upstream `configure`/`make` recipe in `build.sh` | Standalone `oxy-ffmpeg` and `oxy-ffprobe` sidecars |

Initialize the two source submodules after cloning:

```sh
git submodule update --init --recursive
```

Normal Cargo builds compile the linked native libraries automatically once the
platform prerequisites are available. Release Tauri builds additionally prepare
and verify FFmpeg; `pnpm ffmpeg:prepare` exposes that step independently.

Native components must be pinned by version or immutable source revision, built
for every supported target, and recorded in `THIRD_PARTY_NOTICES.md`. They must
not be silently sourced from an untracked developer system installation.
