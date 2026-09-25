# AGENTS.md

Repository-specific rules for coding agents working on OxyViewer.

## Start Here

- Use [CONTRIBUTING.md](CONTRIBUTING.md) for setup, build commands, and the
  verification matrix.
- Use [docs/README.md](docs/README.md) to find the authoritative architecture,
  format, packaging, roadmap, task, and research documents.
- Rust changes must follow [docs/RUST_STYLE.md](docs/RUST_STYLE.md). New
  workspace crates must use `[lints] workspace = true`; do not add crate-wide
  Clippy exemptions.
- Before changing browsing, media caches, request queues, scheduling, or queue
  diagnostics, read
  [docs/PERFORMANCE_INVARIANTS.md](docs/PERFORMANCE_INVARIANTS.md).

## Non-Negotiable Contracts

- Opening a folder must remain cheap, non-recursive, and paged. Defer indexing,
  metadata, preview generation, and decode work from the interactive open path.
- Keep `apps/desktop/src-tauri` thin. Put reusable behavior in the appropriate
  Rust crate. Wiring is the exception: opening the store and building one handle
  per domain crate is the application's job, because there is no crate left that
  could do it without depending on all of them.
- Put public serialized Rust contracts in `oxy-domain` with camelCase serde
  names. Keep frontend IPC wrappers in `apps/desktop/src/lib/api.ts`, updating
  TypeScript types and browser-demo behavior with command changes.
- Never send image bytes through JSON IPC. Use paths, protocols, projections,
  or the existing cache boundaries.
- Keep blocking and CPU-heavy media work off async and UI threads. Preserve
  priority, cancellation, generation, invalidation, lease, and stale-result
  semantics when replacing an implementation.
- Treat the SQLite library and generated previews as rebuildable caches, but
  only where the declaration says so. A table declares itself `cache` (derived;
  a clear may empty it), `user` (typed, named, confirmed, or assigned; a clear
  must never reach it), or `marker` (an allocator or cache-format version) in
  `oxy-store/src/schema`. Never delete user data from a cache clear, cache
  migration, or re-index, and never add a deletion path that is not an explicit,
  confirmed user action.
- Keep storage mechanism out of domain code. `oxy-store` owns the SQLite file,
  its schema, and the statements over it: connections, WAL readers, the
  declaration macro, every `CREATE TABLE`, the migrations that upgrade a file
  written by an earlier build, and `oxy-store/src/repo` — one function per
  statement, taking the connection or the transaction a policy layer opened. A
  domain crate owns what the rows mean. Do not open a database from a domain
  module, do not keep a statement that a repository function should own, and do
  not put a domain rule — a tag cycle, a review revision — into `oxy-store`.
- A repository function opens no transaction and decides nothing. "These four
  writes are one atomic act" and "a move inside its folder keeps the person
  source" are rules: they stay in the crate that owns the domain, which opens
  the transaction and calls repositories inside it. Statements that read or
  write two domains at once live in `oxy-store/src/repo/cross.rs`, because no
  domain crate may own one without depending on a sibling. That is also where
  the two acts no single domain can perform live: `relocate_asset` (a file
  moved) and `forget_asset` (a file is gone), each of which carries tag rows,
  person rows, and cached features in one transaction.
- A domain crate must not depend on `rusqlite`. It names
  `oxy_store::{Connection, Transaction}` — both are re-exported for exactly
  this — and asks the error (`StoreError::is_constraint_violation`) rather than
  matching the engine. `oxy-tags/src/audit.rs` and `oxy-people/src/audit.rs`
  assert this, and assert that each crate carries no SQL of its own.
- Domain crates are siblings: `oxy-library`, `oxy-tags`, and `oxy-people` do not
  depend on one another, so a rule of one cannot name a rule of another. Where
  they need each other's rows, they call a repository function. The application
  is the composition root: it opens the `oxy_store::Store` once and hands it to
  every domain crate (`Library::with_store`, `Tags::new`, `People::new`), which
  is the few lines of wiring `src-tauri` is allowed to own. A domain crate's
  tests may drive a sibling through a `[dev-dependencies]` edge — `oxy-tags`
  does, to prove an identity's claim lands as a `person` tag source — but no
  production dependency may cross.
- Both axes of the person domain are `oxy-people`'s: the identity, the reviews,
  the history, and the references are `user` tables, while the detections, the
  feature vectors, and the analysis-run ledger are `cache` tables. The crate
  holds them together precisely so that emptying the second cannot be mistaken
  for a reason to touch the first.
- Declare every table once in `oxy-store/src/schema`, with its class and how it
  is created. One declaration produces the `CREATE TABLE`, the list of owned
  tables, and the `DELETE` statements, so a new table cannot be created without
  also being cleared. Declare in creation order; clearing walks that order
  backwards so a referencing table is always emptied first. Use `external` when
  something other than the declaration creates the table (FTS5, a backfill
  transaction), and `marker` for an allocator or a cache-format marker.
- Deletion follows ownership, not a list. `oxy-store::schema` derives each
  table's class from that table's own declaration and only audits the result.
  Never delete by table-name prefix and never drop the database.
- Read a result row by column name (`row.get("path")`), never by position
  (`row.get(0)`). Reordering a `SELECT` list must not silently shift values.
- A module may read across the cache/user boundary but never write across it.
  Call a named function on the owning module (for example
  `cache::index::forget_root`) instead of writing SQL against another
  namespace's tables. A source-level test in `oxy-library/src/audit.rs` fails
  the build's tests on any cross-namespace write, and a second one asserts that
  `oxy-library/src/user` carries no SQL of its own. The person namespaces now
  sit outside this crate, so what the pair still guards is the favourites
  against the browsing, index, and projection state. Once the cache namespace
  follows the user namespace out of `oxy-library` (stage E of the split plan)
  the compiler replaces both.
- Route file discovery and safe file operations through `oxy-fs`; do not add
  ad hoc frontend filesystem access.
- Treat native dependencies as pinned inputs. Do not change submodules, the
  pinned vcpkg revision, the pinned native source manifests (`3rdpart/ffmpeg`,
  `3rdpart/libheif`), or third-party notices unless the task requires it.

## Change Discipline

- Keep changes scoped and preserve unrelated worktree edits.
- Add focused tests near changed behavior. Do not hand-edit generated outputs
  such as `target`, `apps/desktop/dist`, `*.tsbuildinfo`, generated Tauri
  schemas, or generated icons.
- Update the current document selected through `docs/README.md` when changing
  architecture, format support, performance behavior, native dependencies, or
  roadmap status. Keep dated measurements under `docs/research` and completed
  plans under `docs/archive`.
- Run the applicable checks from `CONTRIBUTING.md`. Cross-boundary changes need
  both frontend and Rust verification.
- For performance-sensitive media or browsing work, use a release build and a
  representative real fixture, then compare against
  [docs/PERFORMANCE.md](docs/PERFORMANCE.md). Build success alone is not a
  performance or visible-behavior verification.
- Close any debug application instance started for verification when finished.
