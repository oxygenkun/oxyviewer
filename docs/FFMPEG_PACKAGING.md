# Bundled FFmpeg

OxyViewer ships standalone `oxy-ffmpeg` and `oxy-ffprobe` programs built from
unmodified FFmpeg 8.0.1. The application does not link FFmpeg libraries.
The source URL and SHA-256 are pinned in `3rdpart/ffmpeg/source.json`.

## Building installers

Use the repository wrapper, from the repository root:

```sh
pnpm tauri build
# macOS app only:
pnpm tauri build --bundles app
# Windows MSI only:
pnpm tauri build --bundles msi
```

`apps/desktop/scripts/tauri.mjs` prepares FFmpeg before `build`/`bundle` and
merges `tauri.bundle.json`. Do not bypass this wrapper with `pnpm exec tauri`
for release builds. The overlay keeps plain Cargo checks and `tauri dev`
independent of the FFmpeg source build. `pnpm ffmpeg:prepare` explicitly
prepares and verifies the sidecars without building the application.

Prerequisites: Node, Rust, curl, tar with xz support, Bash, GNU Make and a C
compiler; x86_64 builds also need NASM for SIMD. macOS uses Xcode command-line
tools. Linux uses the native compiler. Windows uses MSYS2 UCRT64 with
`make diffutils nasm mingw-w64-ucrt-x86_64-gcc`; set `OXY_FFMPEG_BASH` to its
`usr/bin/bash.exe` if MSYS2 is not installed at `C:/msys64`. Git Bash is not
a substitute. CI installs these prerequisites.

Supported native targets: macOS arm64/x86_64, Windows x86_64 MSVC application
(standalone FFmpeg compiled with MinGW UCRT), Linux arm64/x86_64 GNU.
Cross-compilation and macOS universal builds fail explicitly rather than
packaging a host-architecture executable. Set `OXY_FFMPEG_JOBS` to change the
default four build jobs.

The first build downloads and verifies the official source archive. Build
outputs and the archive live under `target/ffmpeg/<target>`. The staged
executables in `apps/desktop/src-tauri/binaries` have Tauri target suffixes;
installed executables are named `oxy-ffmpeg[.exe]` / `oxy-ffprobe[.exe]` beside
the application binary. Generated files are ignored by Git. Subsequent builds
reuse the cache only when the source/recipe/target and binary checksums match;
capability and encode smoke checks still run on cache hits.

## Configuration and distribution materials

`3rdpart/ffmpeg/build.sh` disables external library autodetection, network
protocols, GPL, nonfree and version3 components. FFmpeg libraries are statically
linked into the two standalone LGPL programs, not into OxyViewer. Windows also
links compiler runtime support statically. System OS libraries remain dynamic.
SIMD remains enabled; unrelated codecs and protocols are omitted.

The enabled features cover the existing HEIF path: MOV/HEIF demuxing, HEVC and
MJPEG decoding, JPEG/BMP encoding, file/pipe protocols, image outputs, scaling,
rotation/flips, crop, format conversion, unsharp and xstack. Rawvideo input and
split support the portable smoke test. Any new backend flags must be reflected
in the build recipe and verification.

Every package includes `licenses/ffmpeg` under its Tauri resources directory:

- The exact source archive, LGPL text and upstream license description.
- The source manifest and build scripts.
- Generated `config.h`, configure summary and version/build information.

Keep these materials in redistributed installers. The included source and
recipe allow rebuilding the standalone FFmpeg programs. Update
`THIRD_PARTY_NOTICES.md` alongside version/configuration changes. No GPL or
nonfree third-party codec library is enabled by this recipe.

## Runtime resolution

`oxy-media` resolves the pair once. `OXY_FFMPEG_DIR` explicitly overrides the
directory for both programs (either the `oxy-` names or standard names).
Otherwise the installed `oxy-` pair is used. An incomplete installed pair fails
as a pair; it never mixes a bundled tool with an unrelated system executable.
Release builds do not use PATH/Scoop fallback. Debug builds retain system-tool
discovery when there is no bundled pair. No process is started during folder
opening solely to locate the programs.

To test a prepared pair outside a Tauri build, copy the target-suffixed files
into a temporary directory as `oxy-ffmpeg[.exe]` and `oxy-ffprobe[.exe]`, then
set `OXY_FFMPEG_DIR` to that directory. A Tauri build already places these names
beside the built application.

## Verification

Preparation checks both versions, license flags, required capabilities and
executes the composition/rotation/sharpening/scale pipeline to JPEG and BMP
with PATH cleared. CI extracts MSI, DEB and AppImage packages and checks the
macOS app directly, verifying the installed pair and bundled source/license
payload before uploading artifacts:

```sh
node 3rdpart/ffmpeg/prepare.mjs --verify-bundle target/release/bundle/macos/OxyViewer.app
```

The synthetic smoke test does not prove camera HEIF compatibility. Run real
HIF regression tests using the installed pair and the private fixture:

```sh
OXY_FFMPEG_DIR="$PWD/target/release/bundle/macos/OxyViewer.app/Contents/MacOS" \
  OXY_HIF_FIXTURE="$PWD/tests/fixtures/DSC00449.HIF" cargo test -p oxy-media
```

CI does not contain the private camera fixture; those tests can skip there.
The installed application should also be checked on each release platform
without a system FFmpeg installation. macOS signing/notarization must include
the external binaries via Tauri's normal signing workflow.

### Local verification, 2026-09-10

- macOS arm64: native source build and cached preparation both passed. The two
  stripped executables total about 7 MiB; the included source archive is about
  11 MiB. `otool -L` reports only Apple system libraries/frameworks.
- `pnpm tauri build --bundles app` produced an app containing both executables
  and all source/license materials; `--verify-bundle` passed with PATH cleared
  for the executable checks. No debug application was launched.
- TypeScript checks, all 167 frontend tests and the frontend build passed.
  Rust formatting and workspace Clippy passed. Real HIF FFmpeg decode,
  orientation, JPEG output and DCT compatibility tests passed.
- The full Rust suite found two pre-existing macOS preview-size failures:
  `app_interim_hif_can_upgrade_to_a_satisfied_preview` and
  `heif_without_identified_fast_representation_uses_semantic_preview_size`.
  Both were reproduced after restoring the original FFmpeg backend code for
  comparison. With those two tests excluded, all 530 remaining unit tests and
  two doctests passed (existing ignored fixture tests remained ignored).
- Windows/Linux package extraction checks are configured in CI; those native
  builds and installed-app behavior have not been executed on this Mac.
