#!/usr/bin/env bash
set -euo pipefail

# Invoked by prepare.mjs. On Windows use MSYS2 UCRT64, not Git Bash.
target=$1
source_dir=$2
build_dir=$3
if command -v cygpath >/dev/null 2>&1; then
  source_dir=$(cygpath -u "$source_dir")
  build_dir=$(cygpath -u "$build_dir")
fi
mkdir -p "$build_dir"
cd "$build_dir"
platform=()
executable_suffix=
case "$target" in
  x86_64-pc-windows-msvc)
    platform=(--target-os=mingw32 --arch=x86_64 --cc=gcc --cxx=g++ --extra-ldflags=-static)
    executable_suffix=.exe
    ;;
  aarch64-apple-darwin|x86_64-apple-darwin)
    export MACOSX_DEPLOYMENT_TARGET=11.0
    platform=(--cc=clang)
    ;;
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
  *) echo "Unsupported FFmpeg target: $target" >&2; exit 1 ;;
esac

# Static FFmpeg libraries are linked only into these standalone LGPL programs.
# Disable autodetection to prevent host libraries changing the shipped license
# or introducing undeclared DLL/dylib dependencies. Keep SIMD enabled.
bash "$source_dir/configure" \
  --disable-autodetect --disable-everything --disable-network \
  --disable-gpl --disable-nonfree --disable-version3 \
  --disable-shared --enable-static --disable-debug --disable-doc \
  --disable-ffplay --enable-ffmpeg --enable-ffprobe \
  --disable-avdevice \
  --enable-avcodec --enable-avformat --enable-avfilter --enable-swscale \
  --enable-decoder=hevc,mjpeg,rawvideo --enable-parser=hevc \
  --enable-demuxer=mov,rawvideo --enable-protocol=file,pipe \
  --enable-encoder=mjpeg,bmp --enable-muxer=image2,image2pipe \
  --enable-filter=buffer,buffersink,scale,format,crop,transpose,hflip,vflip,unsharp,xstack,split \
  "${platform[@]}" | tee configure-summary.txt
make -j "${OXY_FFMPEG_JOBS:-4}" "ffmpeg${executable_suffix}" "ffprobe${executable_suffix}"
