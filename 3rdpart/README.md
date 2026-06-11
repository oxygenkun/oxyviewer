# Third-Party Native Components

This directory contains reproducible native source and packaging definitions
for third-party libraries, managed as git submodules.

LibRaw 0.22.1 is included as a submodule under `3rdpart/libraw` and compiled by
`crates/oxy-media/build.rs`.

Planned additions include libheif and an independent ExifTool worker.

Native components must be pinned by version, built for every supported target,
and recorded in `THIRD_PARTY_NOTICES.md`. They must not be silently sourced from
a developer's system installation.
