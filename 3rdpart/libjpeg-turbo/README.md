# libjpeg-turbo 3.1.3

Unmodified official source archive, statically linked for background JPEG DCT
coefficient stitching. Source and build instructions:
https://github.com/libjpeg-turbo/libjpeg-turbo/releases/tag/3.1.3
https://github.com/libjpeg-turbo/libjpeg-turbo/blob/3.1.3/BUILDING.md

Archive SHA-256:
`075920b826834ac4ddf97661cc73491047855859affd671d52079c6867c1c6c0`

`oxy-media/build.rs` extracts the pinned archive under Cargo OUT_DIR, builds only
`jpeg-static` in Release mode with SIMD required, and compiles the OxyViewer C
adapter separately. No configure-time download or runtime DLL is needed. Requires
CMake and a C compiler; x86/x64 also requires NASM on PATH (or set `NASM` to its
executable path). ARM SIMD uses the native compiler. CI installs NASM explicitly.

Upstream license notices are in LICENSE.md and README.ijg. The archive contains
all original notices and source; modifications belong in the OxyViewer adapter,
not inside the archive. Updating the library requires updating this version,
hash, build.rs, and THIRD_PARTY_NOTICES.md together.
