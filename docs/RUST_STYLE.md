# Rust Style and Lint Policy

OxyViewer targets Rust 2024 with a minimum supported Rust version (MSRV) of
1.85. Rust code must follow `rustfmt` and pass the workspace Clippy policy.

## Required checks

Run these commands from the repository root before submitting Rust changes:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks. `-D warnings` covers compiler warnings in addition to
the lint levels declared in the workspace manifest.

## Policy

- Every workspace package inherits `[workspace.lints]`; new packages must add
  `[lints] workspace = true`.
- The default Clippy lint set is denied. Do not disable Clippy for an entire
  crate or module.
- Debug macros and placeholder implementations (`dbg!`, `todo!`, and
  `unimplemented!`) must not reach committed code.
- Use the Rust 2018+ module layout: define `foo` in `foo.rs` and keep its child
  modules in `foo/`. Do not add `foo/mod.rs`.
- Prefer `let ... else` for early exits, captured identifiers in format strings,
  direct method references over redundant closures, and `map_or`/`map_or_else`
  over a `map(...).unwrap_or(...)` chain.
- Avoid redundant cloning. When cloning an `Arc` or `Rc` intentionally, prefer
  `Arc::clone(&value)` or `Rc::clone(&value)` where that makes shared ownership
  clearer; this is guidance rather than a blanket lint because concise clones
  are sometimes clearer in application wiring.
- Operations inside an `unsafe fn` still require explicit `unsafe` blocks.
  Keep each block small and document the invariant that makes it sound.
- Add a narrow lint exception only when changing the underlying code would make
  it less correct or less readable. Put the exception on the smallest item and
  include a comment explaining why it exists.
- Do not enable all of `clippy::pedantic` or `clippy::nursery`. Adopt individual
  lints only after the workspace is clean under them and their signal is useful
  for this codebase.

`rustfmt.toml` fixes formatting to the Rust 2024 style edition and enables field
and `?` shorthand. Formatting choices should be changed centrally, not through
local `rustfmt::skip` attributes except for data tables where alignment is part
of readability.
