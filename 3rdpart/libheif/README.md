# libheif build recipe

This directory owns OxyViewer's pinned libheif source manifest, its license text
and the scripts that build it. Generated source, objects and libraries are not
checked in. macOS and Linux build it with the pinned FFmpeg decoder through
`pnpm native:prepare` or `pnpm tauri build`; Windows takes the same libheif
version from the vcpkg revision that `.github/workflows/ci.yml` checks out and
decodes HEVC through libde265 instead.

See [the native component inventory](../README.md) and
[format support](../../docs/FORMAT_SUPPORT.md) for how the linked library is used.

## Rebuilding the linked library

The installed `licenses/libheif` directory includes this recipe, the pinned
source manifest and the upstream license text. Download the corresponding
`libheif-1.23.4.tar.gz` source from the URL in `source.json`; its SHA-256 must
match `source.json`. Build the pinned FFmpeg static prefix first
(`pnpm ffmpeg:prepare`), then run:

```sh
tar -xf libheif-1.23.4.tar.gz
bash build.sh aarch64-apple-darwin "$PWD/libheif-1.23.4" "$PWD/build" \
  "$PWD/../native/ffmpeg" "$PWD/../native/libheif"
```

Replace the target with the native target listed in
[CONTRIBUTING.md](../../CONTRIBUTING.md). The install prefix is what
`libheif-sys` resolves through `pkg-config`, so it must match the path recorded
in the application's `.cargo/config.toml` (`target/native/libheif`).

Only the FFmpeg decoder is enabled; every other codec backend is disabled so the
static prefix has no undeclared codec or license dependencies. The build also
patches the generated `libheif.pc`, because upstream 1.23.4 never records the
FFmpeg dependency in it.

libheif is distributed under the GNU Lesser General Public License version 3.
Retain the included `COPYING`, the corresponding source and this build material
when redistributing the linked library, and keep the relink material available
for the application packages that link it. Refer to the upstream license for
terms.
