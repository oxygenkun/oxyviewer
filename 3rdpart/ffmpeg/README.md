# FFmpeg build recipe

This directory owns OxyViewer's pinned FFmpeg source manifest and the scripts
that build and verify the standalone FFmpeg/ffprobe programs. Generated source,
objects and executables are not checked in. From the repository root, run
`pnpm ffmpeg:prepare` or `pnpm tauri build`.

See [the packaging guide](../../docs/FFMPEG_PACKAGING.md) for prerequisites,
runtime lookup and installer verification.

## Rebuilding the distributed standalone programs

The installed `licenses/ffmpeg` directory includes this recipe, the pinned
source manifest and the effective configuration. Download the corresponding
`OxyViewer_<version>_FFmpeg_8.0.1_source.tar.xz` asset from the GitHub Release
matching the installed OxyViewer version. Its SHA-256 must match `source.json`.
Copy it into this directory, then install the native compiler, GNU Make, Bash
and tar (plus NASM on x86_64). For Windows use MSYS2 UCRT64 with its GCC
toolchain. Extract the archive and run:

```sh
tar -xf ffmpeg-8.0.1.tar.xz
bash build.sh aarch64-apple-darwin "$PWD/ffmpeg-8.0.1" "$PWD/build"
```

Replace the target with the native target listed in the packaging guide or
`build-info.json`. Windows commands should run in an MSYS2 UCRT64 shell. The
outputs are `build/ffmpeg[.exe]` and `build/ffprobe[.exe]`; OxyViewer renames these
to `oxy-ffmpeg[.exe]` and `oxy-ffprobe[.exe]`. The two programs are separate from
the OxyViewer application and can also be selected using `OXY_FFMPEG_DIR`.

The recipe disables external dependency autodetection and enables no GPL,
nonfree or version3 components. Its license is LGPL 2.1 or later; retain the
included `COPYING.LGPLv2.1`, `LICENSE.md`, corresponding source and build material
when redistributing these programs. Refer to the upstream license for terms.
