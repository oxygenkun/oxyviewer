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

The project is still pre-release. Cross-platform release qualification, the full
camera fixture matrix, filesystem watching, cancellation for native APIs without
an interrupt hook, and release hardening remain incomplete. The measured
100k-file first-page path also does not yet meet its 300 ms performance budget.

## Documentation

- Use [CONTRIBUTING.md](CONTRIBUTING.md) for development prerequisites, build
  commands, verification, and pull-request guidance.
- Use [the documentation index](docs/README.md) to find current architecture,
  performance, format, packaging, task-plan, and research documents.

## License

OxyViewer is dual-licensed. You may use it under the
[GNU Affero General Public License version 3](LICENSE-AGPL-3.0), or obtain a
separate [OxyViewer Commercial License](LICENSE-COMMERCIAL.md) for proprietary
or other uses that do not comply with the AGPL. Commercial use is permitted
under the AGPL when all AGPL conditions are satisfied.

Commercial licensing is issued by the repository owner and copyright holder;
contact [oxygenkun.1@gmail.com](mailto:oxygenkun.1@gmail.com). Separately
licensed and third-party components retain their own license terms.
