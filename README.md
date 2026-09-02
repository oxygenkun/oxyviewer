# OxyViewer

> Warning: This is an experiment and exercise in fully AI-built Rust project

OxyViewer is a local-first photo browser and organizer built with Rust, Tauri 2,
and React. It opens folders immediately and builds useful caches in the
background instead of requiring a blocking import.

The current implementation establishes the Phase 0 foundation and a functional
Phase 1 directory browser:

- Native folder picker and paged Rust directory scanning
- Grid, list, and loupe workspaces
- Search, type filtering, sorting, selection, and keyboard navigation
- Metadata inspector foundation and safe file-operation contracts
- SQLite library/index foundation and documented native media boundaries

## Development

```bash
pnpm install
pnpm icons
pnpm check
cargo test --workspace
pnpm tauri dev
```

For browser-only UI development, use `pnpm dev`. The frontend detects when it
is outside Tauri and supplies a small demonstration folder.

See [docs/ROADMAP.md](docs/ROADMAP.md) for implementation status.
