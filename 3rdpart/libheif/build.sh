#!/usr/bin/env bash
set -euo pipefail

# Invoked by prepare.mjs. Builds the pinned libheif as a static library for
# macOS/Linux with the FFmpeg decoder enabled, linked against the pinned FFmpeg
# static prefix. Windows takes libheif (libde265 decoder) from the pinned vcpkg
# revision instead and never calls this script.
target=$1
source_dir=$2
build_dir=$3
ffmpeg_prefix=$4
install_prefix=$5

case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin)
    # Keep the C++ runtime and deployment target aligned with the FFmpeg prefix.
    export MACOSX_DEPLOYMENT_TARGET=11.0
    ;;
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
  *) echo "Unsupported libheif target: $target" >&2; exit 1 ;;
esac

# Resolve every path before CMake consumes it: a relative FFMPEG_ROOT is
# resolved against the source directory and silently finds the wrong headers.
# The build and install directories must also exist before `tee` opens its log.
mkdir -p "$build_dir" "$install_prefix"
source_dir=$(cd "$source_dir" && pwd)
build_dir=$(cd "$build_dir" && pwd)
ffmpeg_prefix=$(cd "$ffmpeg_prefix" && pwd)
install_prefix=$(cd "$install_prefix" && pwd)

# Only the FFmpeg decoder is compiled in. Coding formats the application does
# not discover stay out so the static prefix has no undeclared codec or license
# dependencies; HEVC (and the JPEG/AV1 items the FFmpeg decoder also handles)
# comes from the pinned FFmpeg libraries.
cmake -S "$source_dir" -B "$build_dir" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$install_prefix" \
  -DBUILD_SHARED_LIBS=OFF \
  -DBUILD_TESTING=OFF -DWITH_EXAMPLES=OFF -DBUILD_DOCUMENTATION=OFF \
  -DWITH_EXAMPLE_HEIF_THUMB=OFF -DWITH_EXAMPLE_HEIF_VIEW=OFF \
  -DFFMPEG_ROOT="$ffmpeg_prefix" \
  -DWITH_FFMPEG_DECODER=ON \
  -DWITH_LIBDE265=OFF \
  -DWITH_AOM_DECODER=OFF -DWITH_AOM_ENCODER=OFF -DWITH_DAV1D=OFF \
  -DWITH_X265=OFF -DWITH_X264=OFF \
  -DWITH_JPEG_DECODER=OFF -DWITH_JPEG_ENCODER=OFF \
  -DWITH_OpenJPEG_DECODER=OFF -DWITH_OpenJPEG_ENCODER=OFF \
  -DWITH_RAV1E=OFF -DWITH_SvtEnc=OFF -DWITH_KVAZAAR=OFF \
  -DWITH_OPENJPH_ENCODER=OFF -DWITH_UVG266=OFF -DWITH_VVDEC=OFF -DWITH_VVENC=OFF \
  -DWITH_OpenH264_DECODER=OFF -DWITH_LIBSHARPYUV=OFF \
  -DWITH_UNCOMPRESSED_CODEC=OFF -DWITH_GDK_PIXBUF=OFF \
  -DENABLE_PLUGIN_LOADING=OFF | tee "$build_dir/configure-summary.txt"
cmake --build "$build_dir" -j "${OXY_LIBHEIF_JOBS:-4}"
cmake --install "$build_dir"
