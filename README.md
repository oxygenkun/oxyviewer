# OxyViewer

> Warning: This is an experimentally, fully AI-built Tauri project

OxyViewer is a local-first photo browser and organizer built with Rust, Tauri 2,
React 19, and TypeScript. It opens a folder without a blocking recursive import,
then schedules indexing, metadata, and generated previews behind the interactive
browsing path.

The current application includes:

- Persistent library roots, non-recursive folder browsing, an on-demand directory
  tree, and background SQLite indexing/search
- Virtualized grid and list views, a progressive loupe and filmstrip, multi-select,
  keyboard navigation, search, rating/color/type filters, and sorting
- Generated and cached RAW/HEIF/TIFF previews, full-detail RAW and HEIF paths,
  color management, and scoped priority scheduling
- A native in-process metadata engine for EXIF/XMP/IPTC/ICC/MakerNotes, Sony
  shooting-focus overlays, and sidecar-first rating/color editing
- Configurable preview-cache storage and an optional, checksum-verified ExifTool
  capability for explicit embedded metadata synchronization
- Rename, copy, move, reveal-in-file-manager, and recoverable trash workflows

The project is still pre-release. Cross-platform codec packaging, the full camera
fixture matrix, filesystem watching, cooperative mid-decode cancellation, and
release hardening remain incomplete. The measured 100k-file first-page path also
does not yet meet its 300 ms performance budget.

## Development

Native builds require CMake and a C/C++ compiler. x86/x64 builds also require
NASM on `PATH` (or the `NASM` environment variable pointing to its executable)
for the statically linked libjpeg-turbo SIMD backend. See
[the pinned dependency notes](3rdpart/libjpeg-turbo/README.md).

```bash
pnpm install --frozen-lockfile
pnpm dev                 # browser-only demo
pnpm tauri dev           # native desktop app
```

Common verification commands are `pnpm check`, `pnpm test`, `pnpm build`,
`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
and `cargo test --workspace`. Use `pnpm icons` only when intentionally
regenerating application icons.

Start with [the architecture overview](docs/ARCHITECTURE.md), then see the
[roadmap](docs/ROADMAP.md), [format matrix](docs/FORMAT_SUPPORT.md), and
[performance budgets](docs/PERFORMANCE.md) for the current implementation limits.
