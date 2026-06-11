# Native Runtime Components

This directory contains reproducible native source and packaging definitions
for LibRaw, and will contain the planned libheif and independent ExifTool
worker definitions.

Native components must be pinned by version and checksum, built for every
supported target, and recorded in `THIRD_PARTY_NOTICES.md`. They must not be
silently sourced from a developer's system installation.

LibRaw 0.22.1 is vendored under `native/libraw/0.22.1` and compiled by
`crates/oxy-media/build.rs`.
