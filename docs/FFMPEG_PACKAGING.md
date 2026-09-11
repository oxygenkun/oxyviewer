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

For a local Windows machine, install the same toolchain from PowerShell before
the first release build:

```powershell
winget install --id MSYS2.MSYS2 --exact --source winget --location C:\msys64 --silent --accept-package-agreements --accept-source-agreements
$env:MSYSTEM = 'UCRT64'
$env:CHERE_INVOKING = '1'
# Use separate shells: a core runtime update can terminate the first shell.
& C:\msys64\usr\bin\bash.exe --login -c 'pacman --noconfirm -Syuu'
& C:\msys64\usr\bin\bash.exe --login -c 'pacman --noconfirm -Syuu'
& C:\msys64\usr\bin\bash.exe --login -c 'pacman --noconfirm -S --needed make diffutils nasm mingw-w64-ucrt-x86_64-gcc'
pnpm tauri build
```

The two-stage update follows the [MSYS2 setup instructions](https://www.msys2.org/docs/ci/#other-systems).
The build wrapper selects UCRT64 itself, so subsequent builds work from a fresh
PowerShell without adding MSYS2 to the global PATH. `OXY_FFMPEG_DIR` only changes
runtime discovery; it does not provide the source-build prerequisites.

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

### Windows standard build verification, 2026-09-11

- Installed and updated MSYS2 at the default location, then installed Make,
  diffutils, NASM and the UCRT64 GCC toolchain. The local mirror priority was
  adjusted to an accessible mirror from MSYS2's supplied list after the primary
  servers timed out.
- Fixed the build recipe's Windows Make targets: MinGW requires `ffmpeg.exe`
  and `ffprobe.exe`, whereas macOS/Linux retain the suffix-free targets.
- The unmodified command `pnpm tauri build` completed the pinned FFmpeg 8.0.1
  source build, frontend/Release build, and both MSI and NSIS installers.
  A subsequent `pnpm ffmpeg:prepare` reused the verified cache successfully.
- MSI administrative extraction followed by `--verify-bundle` passed: both
  programs run with PATH cleared, capabilities/filter smoke tests pass, and the
  exact source archive and required license/build materials are present.
- The application extracted from the MSI passed `cold-preview-hif`,
  `resource-stress-hif` and `filmstrip-scroll-hif` with PATH empty and
  `OXY_FFMPEG_DIR` unset. Cold first preview was 83.2 ms and first tile 1015 ms.
  The stress run switched 80 paths, revisited eight full presentations, and
  passed active-resource reads across cache maintenance. These single runs use
  copies of a real Sony HIF fixture, not a diverse camera corpus. All test
  application instances were closed.
