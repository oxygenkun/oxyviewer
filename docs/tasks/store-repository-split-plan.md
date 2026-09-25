# Store, repository, and domain split

## Goal

Separate three concerns that `oxy-library` currently holds in one crate:

1. **Storage mechanism** — opening the file, WAL, reader fan-out, transactions.
2. **The rows** — table definitions and every SQL statement that reads or writes
   them.
3. **Meaning and rules** — what a tag, a favourite folder, or a person identity
   *is*, and what a user action is allowed to do.

Today (1) is already in `oxy-store`, but (2) and (3) are interleaved inside
`oxy-library`: 10525 lines, of which 4263 are inline tests and 6262 are
production code carrying 373 SQL statements across 11 files.

## Decisions taken

- **`oxy-store` absorbs all three storage layers**: mechanism, table
  definitions, and repositories. Every SQL statement in the workspace lives
  there.
- **Favourites (`library_roots`) stay in `oxy-library`**, which keeps meaning
  "the photo library": the folders the user added, plus the rebuildable index
  over them.
- **Tags become their own crate.**
- **People logic goes into the existing `oxy-people`.**
- **No forwarding shell.** `Library` does not keep pass-through methods for the
  code that leaves it. Every call site moves to the crate that now owns the
  behaviour, and `apps/desktop` changes with it.

## Split criteria

Use these four tests before promoting a module to a crate. A crate boundary
changes the dependency graph, the build configuration, and the public API
surface; none of those are reversible cheaply. A folder gives readability,
which is cheap and adjustable — a crate gives enforcement. Default to a folder,
and promote only when one of these holds.

1. **Independent reuse** — two or more consumers that have nothing to do with
   each other. `oxy-media` decoding is reached by grid thumbnails, loupe
   frames, and two benchmark binaries.
2. **Enforced direction** — the lower layer must not be able to name the upper
   one. This is the only form of the guarantee Rust can give: within one crate,
   `a.rs` writing `use crate::b::X` and `b.rs` writing `use crate::a::Y` is
   legal and silent. This criterion alone justifies `oxy-tags`: `user/tags.rs`
   holds ~445 lines of person logic today, and only a crate boundary makes that
   a compile error.
3. **Independent build configuration** — `[profile.*.package.*]` and
   `[lints]` key on the crate name. The workspace already relies on this:
   `[profile.dev.package.oxy-media] opt-level = 2` cannot be expressed for a
   module. A folder cannot be "decoded at opt-level 2 but CRUD at 0".
4. **Not always edited together** — if the module shares most of its types with
   a neighbour and every change touches both, a boundary only adds API churn.

And one reverse test: **after splitting, will every edit to this crate rebuild
a large downstream?** If yes it is not stable enough to sit low in the graph,
and splitting makes iteration worse. Evidence for why this matters here:
`oxy-domain` is the most-edited crate in the repository (47 commits) and seven
crates covering ~50,000 lines depend on it, while `oxy-media` is edited more
often (67 commits) but only `src-tauri` (7,822 lines) sits below it. The rule
that follows: **order by edit frequency, not by abstraction** — the more
frequently a thing changes, the closer to the leaves it belongs.

Applied to the candidates in this plan:

| Candidate | Reuse | Direction | Build config | Coupled | Verdict |
| --- | --- | --- | --- | --- | --- |
| `oxy-store` | ✓ | ✓ | — | — | correct — zero `oxy-*` dependencies |
| `oxy-media` | ✓ | ✓ | ✓ | — | must stay separate |
| `oxy-runtime` | ✓ | ✓ | — | — | must stay separate |
| tags | — | ✓ | — | — | split |
| favourites (133 lines) | — | — | — | ✗ | **not worth a crate** — stays in `oxy-library`, which is literally "the roots the user added, plus the index over them" |
| `src-tauri/jobs/preview.rs` | — | — | ✓ | — | split, for the reason in stage P below |

## Why "no shell" is affordable

`apps/desktop/src-tauri` is the only caller, and the real call volume is much
smaller than the 83-method API suggests. Measured:

| Destination | Call sites | Files |
| --- | --- | --- |
| `state.people` — identity, review, history, the person↔tag bridge | 31 | `commands/people.rs` (26), `commands/folder.rs` (5) |
| `state.tags` — vocabulary, assignment, XMP sync, tag state on move/copy/delete | 19 | `commands/tags.rs` (14), `commands/folder.rs` (5) |
| `state.library` — favourites | 7 | `commands/library.rs` (6), `commands/folder.rs` (1) |
| `state.library` — index, browsing, projections, metadata projections | 26 | `commands/folder.rs`, `commands/library.rs`, `jobs.rs`, `jobs/{metadata,preview,directory_tree}.rs` |
| **Total** | **83 call sites, 59 distinct methods** | **7 files** |

The `state.people` and `state.tags` rows both landed: 31 call sites in stage D,
19 in stage C. The favours and index rows stay on `state.library`, which is why
that handle still exists.

Every command already returns `Result<T, String>` through
`map_err(|error| error.to_string())`, so per-crate error types cost nothing at
the seam: `oxy_tags::TagError` and `oxy_people::PeopleError` replace
`LibraryError` in the commands that move — measured, the 31 person call sites
changed nothing but the receiver.

## 27 of 86 public methods have no caller

Worth deciding before anything moves, because carrying them across multiplies
the work for code nobody runs:

- **Person analysis, 16 methods** — `begin/cancel/claim/complete/fail/enqueue/
  recover/seal_person_analysis*`, `get_person_analysis`, `list_person_detections`,
  `put_person_feature`, `search_person_features`,
  `search_current_detected_person_features`, `person_analysis_is_current`,
  `person_vector_status`, `list_manual_person_anchors`. This is the analysis
  half of the person feature, and it confirms from the other side what
  `oxy-people` having no consumers already suggested: **the pipeline is not
  wired to the application.**
- `clear_rebuildable_cache` — the cache-clear entry point. Still unreachable
  from the UI; the desktop "clear cache" action only clears the preview disk
  cache.
- The rest are superseded or internal-looking variants (`index_root` versus
  `index_root_with_progress_and_directory_scan`, `browse_directory` versus
  `browse_directory_revision`, `contains_root`, `list_directories`).

`open` and `in_memory` are reached as associated functions (`Library::open`),
not through a receiver, so they are live.

**Recommendation: port only what is called, and delete the rest in the same
commit that moves it.** A method with no caller has no behaviour to preserve,
and moving it would add untested policy to a new crate. If the person analysis
path is going to be wired later, it is cheaper to reintroduce it against the new
boundary than to move it blind. If you would rather keep it as-is, say so and it
moves with the rest — but then the 16 methods need tests at the new boundary,
because today nothing exercises them.

**Superseded by stage D.** The option above was taken: the 16 analysis methods
moved as-is, and the tests came with them — `analysis_runs.rs` alone carries 11.
The count of "no caller" methods was measured from `apps/desktop` only, and it
missed that the person WIP line already drives the whole run ledger from
`oxy-people`'s own tests and from the enrollment service in `analysis.rs`. The
cleanup that remains is `clear_rebuildable_cache`, which is still unreachable
from the UI: the desktop "clear cache" action clears the preview disk cache only.
That gap belongs to the desktop, not to this plan.

## Target layout

```
crates/oxy-store/src/
  lib.rs               Store, StoreError, re-exported Connection/Transaction
  table.rs             tables! macro, TableDef, create_all, clear_all
  schema.rs            DataClass, classification, audit tests
  schema/cache.rs      CREATE TABLE for every derived table
  schema/user.rs       CREATE TABLE for every user-owned table
  repo.rs              repository root
  repo/library.rs      favourites, index, browsing, projections
  repo/tags.rs         tags
  repo/people.rs       person identity, review, history
  repo/person_cache.rs detections, feature spaces and vectors, analysis runs
  repo/cross.rs        queries that join two domains

crates/oxy-library/    favourites + index + browsing + projections
crates/oxy-tags/       tag vocabulary rules, tree invariants, XMP payload
crates/oxy-people/     person identity, review, history, caches, inference pipeline
crates/oxy-preview/    preview request identity, queue, projection restore
```

The four domain crates are **siblings with no dependencies between them**. That
is what turns the tag↔person direction from a convention into a compiler
check, and it is why `repo/cross.rs` has to exist: `filter_assets_by_tags`
joins `indexed_assets` (library) with `asset_tags` (tags), so no domain crate
can own it without depending on a sibling. `filter_assets_by_person` did **not**
need the same treatment: the directory snapshot arrives as an argument, so
`oxy-people` reads its own rows through `repo::people::instance_records` and
intersects them in memory. That is the cheaper answer whenever a caller already
holds the asset list — a join is only required when the rows must be selected
in SQL to keep paging honest.

`oxy-preview` does not touch the database — it depends on `oxy-library` for
projections and `oxy-media` for decoding. Its reason to be a crate is build
configuration, not dependency direction: today its 971 lines of tests cannot
run without compiling and linking Tauri.

### Cross-domain queries get a natural home

`filter_assets_by_tags` joins `indexed_assets` (library) with `asset_tags`
(tags). No domain crate can own it without depending on a sibling. Because
`oxy-store` owns every table, `repo/cross.rs` is the correct place, and
`oxy-tags` exposes the browsing-level function that calls it.

Stage D showed the person filter is the opposite case:
`filter_assets_by_person` takes the directory snapshot as an argument, so it
reads `person_review_decisions` through `repo::people` and intersects in memory,
with no cross-domain statement at all. The general rule: a join is required when
the rows must be chosen in SQL for paging to stay honest; when the caller
already holds the asset list, filtering in Rust across a sibling's rows is both
correct and cheaper.

### Composition moves to the application

Removing the shell leaves nobody to construct `Store` and hand it to three
domains, so `apps/desktop` becomes the composition root:

```rust
pub(crate) struct AppState {
    pub(crate) library: Arc<oxy_library::Library>,
    pub(crate) tags: Arc<oxy_tags::Tags>,
    pub(crate) people: Arc<oxy_people::People>,
    // ...
}
```

No `store` field was needed. `Library` holds the `Arc<Store>` and hands it out
through `store()`, so the composition root reads:

```rust
let library = Arc::new(Library::open(&data_dir.join("oxyviewer.sqlite"))?);
let tags = Arc::new(oxy_tags::Tags::new(library.store()));
let people = Arc::new(oxy_people::People::new(library.store()));
```

Three lines of wiring in `lib.rs`, replacing `Library::open`. `AGENTS.md` says to
keep `apps/desktop/src-tauri` thin; that rule is about behaviour, not wiring,
and the paragraph now says so explicitly. If the wiring grows past a handful of
lines, the alternative is a small composition crate holding the three handles as
public fields — a struct of objects, which is not the same thing as a struct of
forwarders.

`Store::open` gained the schema step (it now owns the DDL): the "user tables
before cache tables" ordering and the vector-extension registration are both
inside `oxy-store`, where the declarations are.

## Declaration change

```rust
oxy_store::table::tables! {
    cache  indexed_assets = "root_path TEXT NOT NULL, path TEXT NOT NULL, ...";
    cache  person_instances_cache = "...";
    user   custom_tags = "...";
    user   person_manual_instances = "...";
    marker library_index_sequence = "id INTEGER PRIMARY KEY CHECK(id = 1), ...";
    external indexed_asset_search;
}
```

- `cache` — derived. Emptying it costs a rebuild. Only `clear` touches it.
- `user` — typed, named, confirmed, or assigned. Never emptied by a clear.
- `marker` — allocator or cache-format marker. Not cleared, not user data.
- `external` — declared here, created elsewhere (FTS5, a backfill transaction).

One declaration still produces the `CREATE TABLE`, the owned-table list, and the
`DELETE`. The class is now a word in the declaration rather than a consequence of
the file's directory, because after this change one schema file declares tables
of all three classes.

Two properties get **stronger**:

- **Creation and clearing order becomes explicit.** User tables are declared
  before cache tables in one file each, so the "user storage is never the first
  thing a cache migration defines" rule and the backwards clearing walk are
  readable in one place instead of inferred from directory layout.
- **Cross-namespace writes stop being an audit.** A test in `schema.rs` greps
  source text today because `cache/` and `user/` are directories in one crate.
  Once the SQL is private to `oxy-store`, a domain crate physically cannot write
  another domain's table. The guarantee moves from a lint to the compiler.

### Where the two axes separate

The current split is *ownership*: `cache` (derived) versus `user` (the user's
words). The new split is *domain*: library, tags, people. They cross, and that
crossing is why the current layout strains:

| | derived (rebuildable) | user-owned |
| --- | --- | --- |
| library | `directory_snapshots`, `indexed_*`, `resource_projections` | `library_roots` |
| tags | — | `custom_tags`, `asset_tags`, `asset_tag_sources`, `asset_tag_xmp_state`, `tag_xmp_sync_queue` |
| people | `person_instances_cache`, `person_features_cache`, `person_analysis_*` | `folder_people`, instances, reviews, events, references, `historical_*`, `person_tag_links`, `person_tag_overrides` |

The person caches therefore live in `repo/people.rs` and are declared `cache`.
Correct, and the first place a future reader will get confused — it needs a
comment saying so.

## The seam, concretely

`set_asset_tag` mixes both concerns in 39 lines. Policy: *assigning by hand
drops a legacy source, adds a manual one, re-derives the effective assignment,
and queues an XMP write.* Statements: four of them.

```rust
// oxy-store/src/repo/tags.rs — the statements
pub fn replace_manual_source(&self, tx: &Transaction, path: &str, tag_id: i64, assigned: bool)
pub fn reconcile_effective(&self, tx: &Transaction, path: &str, tag_id: i64)
pub fn enqueue_sync(&self, tx: &Transaction, paths: &[String])
```

```rust
// oxy-tags/src/lib.rs — the rule
let mut connection = store.write();
let transaction = connection.transaction()?;
if !repo.tag_exists(&transaction, tag_id)? {
    return Err(TagError::MissingTagParent);
}
for path in &paths_stringified {
    repo.replace_manual_source(&transaction, path, tag_id, assigned)?;
    repo.reconcile_effective(&transaction, path, tag_id)?;
}
repo.enqueue_sync(&transaction, &paths_stringified)?;
transaction.commit()?;
```

**The transaction stays with the policy layer**, because "these four writes are
one atomic act" is a rule, not a statement. Repository functions take
`&Transaction` rather than opening their own. Domain crates therefore still need
`Connection` and `Transaction`, which `oxy-store` re-exports.

The checkable invariant is consequently **"domain crates do not depend on
`rusqlite`"**, not "domain crates contain no SQL text" — the latter cannot hold
while policy owns transaction boundaries. A test asserts the dependency rule,
and a second asserts that SQL verbs appear only under `oxy-store/src/repo`.

**Repository return types.** Recommended: repositories return `oxy-domain` types
(`CustomTag`, `HistoricalPerson`) and `oxy-store` gains an `oxy-domain`
dependency. The alternative — repositories return row structs and the domain
crate maps them — doubles the type count and moves 373 row-mapping sites into
the domain crates for no gain, since mapping is not a rule.

## Prerequisite: the worktree has four lines in flight

74 files are uncommitted, and two of them are the ones this plan needs most:

- **The `oxy-store` extraction** (`crates/oxy-store/` untracked, 15 files in
  `crates/oxy-library`, plus `Cargo.toml`, `AGENTS.md`, `CONTRIBUTING.md`, and
  the architecture docs). Verified: 94 tests green, workspace check clean. This
  is the foundation the plan builds on.
- **The person line** (`crates/oxy-people/` untracked, `commands/people.rs`
  untracked, `PersonReviewPanel.tsx`, `components/people/`, `api.ts` +237,
  `types.ts` +55, `PERSON_WORKFLOW.md`, 11 research documents).
- **`oxy-media` / `oxy-fs`** (7 files).
- This plan.

`crates/oxy-library/src/user/tags.rs` carries both the extraction and the
person-line `asset_tag_source_kinds` feature in one file, so the two cannot be
separated by path — the same situation as the two library refactor commits on
2026-09-24. **The `oxy-store` extraction should be committed before stage 0
starts**, because it is verified, self-contained, and this plan extends exactly
those files. Whether the person line is committed, stashed, or left in place is
your call, but `commands/people.rs` — untracked, and 26 of the 83 call sites —
gets rewritten in stage D.

**Resolved by stage D.** The person line *was* the thing stage D landed, so the
last of those files stopped being a prerequisite and became the work. The
pipeline side of `oxy-people` (`alignment.rs`, `face_input.rs`, `onnx_face.rs`,
`artifact_store.rs`, `analysis.rs`) was already written but untracked, and it
entered version control with the identity and cache halves that moved onto it.
The frontend half of the same line (`components/people/`, `PersonReviewPanel.tsx`,
and the `api.ts`/`types.ts` wrappers) is still uncommitted and still the person
line's to land.

## Progress

| Stage | State | Commit |
| --- | --- | --- |
| 0 — extract `oxy-store` | done | `b66602e` |
| A — schema into `oxy-store` | done | `1143b0d` |
| B — repositories for favourites, tags, people | done | `b3c57ea` |
| C — `oxy-tags` extracted, `Library` tags methods deleted | done | `5b1e090` |
| D — `oxy-people` owns the person domain, caches included | done | this commit |
| E, P | not started | |

### What stage B settled

The layers are now where the plan wanted them, and three details changed shape
during the work:

- **Repositories are free functions, not methods on a repository object.** The
  plan sketched `repo.replace_manual_source(&transaction, ...)`; what landed is
  `repo::tags::replace_manual_source(&transaction, ...)`. The `Transaction`
  argument makes a receiver redundant, and a unit struct would have carried no
  state.
- **`oxy-store` gained an `oxy-domain` dependency**, as recommended here. It is
  the only `oxy-*` edge the crate has, and it is one-way: repositories return
  `CustomTag`, `PersonInstance`, `FolderPerson`, `PersonReview`, and so on, so no
  row-mapping code moved into a domain crate.
- **Two types left `oxy-library`.** `ManualPersonAnchor` moved to
  `oxy-domain::person` (re-exported from `oxy-library`, so no consumer path
  changed) because a repository must be able to return it, and
  `PersonReviewDecision` gained `as_str`/`from_text` so the stored spelling of a
  decision has one definition instead of one per layer. `TagXmpPayload` stayed:
  its JSON is parsed in `oxy-library`, so the repository returns the two stored
  strings and the payload is assembled where the `serde_json` error is.
- **Person-source reconciliation is a cross-domain repository.**
  `reconcile_person_source_for_{asset,subject}` read `person_*` and write
  `asset_tag_sources`, so `repo/cross.rs` owns them; `oxy-library` and, later,
  `oxy-tags` and `oxy-people` both call the same functions.
- **Nothing was deleted.** The plan's "port only what is called" applies when a
  method *moves* to a new crate. Stage B moves no call site, so all 86 public
  methods stayed, and the 16 no-caller person-analysis methods are still here to
  be dropped in stage D.

The guard is now two source-level tests in `oxy-library/src/audit.rs`: the
existing cross-namespace check, plus a new assertion that `src/user` carries no
SQL of its own in production code. Only `src/cache` still holds statements
(stage E).

Stage A landed with one deliberate change to the declaration syntax sketched
below. The sketch has four class words (`cache`, `user`, `marker`, `external`),
but `external` is a statement about *creation*, not about meaning:
`asset_tag_sources` is user-owned and created by a backfill transaction, while
`indexed_asset_search` is derived and created by an FTS5 statement. Folding the
two axes into one word would have lost the class of every external table, and
the audit that a clear preserves every user fact reads exactly that class. The
declaration is therefore two words:

```rust
cache  create   indexed_assets = "...";
cache  external indexed_asset_search;
user   create   custom_tags = "...";
user   external asset_tag_sources;
marker create   library_index_sequence = "...";
```

Clearing is derived from the class — `cache` is emptied by a clear, `user` and
`marker` are not — so the old `preserve` keyword is gone rather than renamed,
and the two audit tests that need a live database moved next to the
declarations they check, into `oxy_store::schema`.

### What stage C settled

The tags domain is now a crate of its own and `Library` no longer has a single
tag method. Five things took a different shape than the sketch above.

- **`oxy-tags` carries no dependency on the storage engine.** `oxy-store`
  re-exports `Connection` and `Transaction`, so `Tags` can open the transaction
  without naming `rusqlite`. Constraint rejection is mapped through
  `StoreError::is_constraint_violation()` rather than by matching
  `rusqlite::ErrorCode`, which is what makes "repeated tag name" a
  `TagError::DuplicateTagName` in one place. `parking_lot` is deliberately absent
  too: the crate would compile with it, but the point of the boundary is that a
  domain crate names its store and nothing below it.
- **Four tag↔person bridge methods stayed in `oxy-library`, not `oxy-tags`.**
  `get/set_person_tag_link` and `get/set_person_tag_override` lived in
  `user/tags.rs` but are person-side statements: they begin with
  `historical_person_of_subject(folder, subject)` and only then read or write the
  tag side. Moving them to `oxy-tags` would have required `oxy-tags` to depend on
  `oxy-people`, reversing the direction the split exists to enforce. They are now
  in `user/people.rs`, reading both sides through `repo::*` statements, so
  `oxy-tags` and `oxy-library` still do not know about each other. The plan's
  "`Library` tags methods deleted" is therefore scoped to the vocabulary,
  assignment, XMP-sync, and per-asset tag-state methods — 19 call sites — and
  the four bridges are counted on the person side, where stage D will move them.
- **Two cross-domain statements became cross-domain *acts*** in
  `repo::cross`: `relocate_asset(source, destination, same_folder)` and
  `forget_asset(path)`. A file move is a tag action and a person action at once —
  four writes in a fixed order — so it is one function next to the other
  statements that read both domains. The *rule* that decides `same_folder`
  (`source.parent() == destination.parent()`, pure `std::path`) stays in
  `oxy-tags` as `move_asset_state`, which is why the crate needs no `oxy-fs`
  dependency to move tag state. `repo::people` gained `rename_cached_features`
  and `forget_cached_features` for the same reason: the cached-feature rows are
  person data that a tag-side move has to repoint.
- **`Library` hands out its store instead of wrapping it.** The plan's
  composition-root sketch assumed each domain constructs its own handle; in
  practice the application owns one `Store` and every domain needs *that* one.
  So `Library` holds `Arc<Store>`, gained `with_store(Arc<Store>)` and
  `store()`, and lost the private `from_store`. `lib.rs` now does
  `Tags::new(library.store())`, and `AppState.tags` sits beside
  `AppState.library` as a sibling. `LibraryError` lost `InvalidTagName`,
  `TagHierarchyCycle`, and `DuplicateTagName` with the code that raised them;
  `MissingTagParent` stayed because the person-side bridges still reject a link
  to a tag that does not exist.
- **The guard is now two tests in `oxy-tags/src/audit.rs`.** One reads the
  crate's own `Cargo.toml` and asserts `rusqlite` does not appear before
  `[dev-dependencies]` — the gate this stage was defined by. The other asserts
  that no SQL verb appears under `oxy-tags/src` in production code. The second
  has to skip `audit.rs` itself, since a checker has to spell the verbs it looks
  for; the same reason `oxy-library/src/audit.rs` carries the identical skip.

**Conservation check.** `crates/oxy-library/src` held 86 `#[test]` functions at
`b3c57ea`; 75 remain there and the 11 from `user/tags.rs` run in `oxy-tags`, so
`oxy-tags` reports 13 (11 moved + 2 new audit tests). Measured now: `oxy-store`
12, `oxy-tags` 13, `oxy-library` 75 (73 pass, 2 ignored) — all green.

### What stage D settled

The person domain is a crate of its own, and `oxy-library` no longer holds a
single person table, statement, or method. Six things took a different shape
than the sketch above.

- **The person caches moved with the identity, so stage E lost its person half.**
  The plan put the person caches in `repo/people.rs` and left them to stage E.
  What landed is `oxy-store/src/repo/person_cache.rs` — 35 statements in 34
  functions — next to `repo/people.rs` rather than next to `repo/library.rs`,
  because every table those statements touch is a person table. Filing them
  under "cache" would have made module ownership and table ownership disagree.
  `oxy-people` now holds `detections.rs`, `features.rs`, and `analysis_runs.rs`,
  so the crate owns both axes of the person domain: `user` (identity, reviews,
  history, references) and `cache` (detections, vectors, run ledger). Stage E is
  therefore library caches only.
- **`Library` lost its person surface entirely.** `crates/oxy-library/src/cache/
  {analysis,detections,features}.rs` and `user/people.rs` were moved, not copied:
  the four tag↔person bridge methods that stage C deliberately left in
  `user/people.rs` are now `oxy-people/src/identity.rs`, which is where the plan
  said they would end up. `LibraryError` dropped the nine person variants it no
  longer raises — including `MissingTagParent`, which now exists only as
  `TagError::MissingTagParent` and `PeopleError::MissingTagParent`, one per side
  that can actually raise it. `Library` also lost its `vector_status` field: the
  vector extension's state is the person domain's business, and `People::new`
  copies it from the store for exactly the same reason `Library` used to.
- **Two types left `features.rs` for `oxy-domain`.** The repository has to
  return and accept them, so `FeatureModality` (with the `as_str`/`from_text`
  pair, so the stored spelling of "face" has one definition) and
  `PersonFeatureMatch` moved up. `PersonFeature` and `PersonFeatureSearch`
  stayed: the latter borrows for its whole life and is a call shape, not a
  vocabulary. `DetectionStageContext` stayed `pub(crate)`.
- **The scan the enrollment step needs became cancellable in `oxy-fs`.** This is
  the one prerequisite the plan did not anticipate. `analysis.rs` registers a
  whole folder before writing anything, so it must be able to stop during
  enumeration and must not publish a partial snapshot when it does;
  `oxy-fs` gained `FsError::Cancelled` and `scan_assets_with_progress_and_cancel`,
  with `scan_assets_with_progress` delegating to it with `|| false`. It is its
  own commit, because it is a change to a crate that has nothing to do with the
  person domain.
- **`oxy-people`'s pipeline needed the store, not the library.** `Cargo.toml`
  gained `oxy-store` and dropped its `[dev-dependencies] oxy-library`. The crate
  no longer reaches the database through a sibling; it holds the same
  `Arc<Store>` every other domain holds. `People::new` is the whole constructor.
- **`oxy-tags` gained a `[dev-dependencies]` edge to `oxy-people`.** Two of its
  tests prove that an identity's claim lands as a `person` tag source, and they
  do it by driving the real policy — create a folder person, create an instance,
  review it, bind a tag. Reproducing that with raw repository calls would have
  re-implemented the identity policy inside a test of the tag side. A
  dev-dependency crosses a boundary that a production dependency may not, and
  the audit test that reads `[dev-dependencies]` still passes because it only
  forbids `rusqlite` there.

**Conservation check.** `crates/oxy-library/src` held 75 `#[test]` functions at
`5b1e090`; the four moved files carried 24 of them (`cache/analysis.rs` 11,
`cache/features.rs` 6, `user/people.rs` 5, `cache/detections.rs` 2), so 51
remain. `oxy-people` reports 46: its 18 pipeline tests, those 24, and 2 new
audit tests. Measured now: `oxy-store` 12, `oxy-tags` 13, `oxy-library` 51
(49 pass, 2 ignored), `oxy-people` 46 (44 pass, 2 ignored) — all green, and
`cargo check --workspace --all-targets` is clean.

## Stages

| Stage | Work | Touches | Gate |
| --- | --- | --- | --- |
| A | `schema` into `oxy-store`; explicit class in `tables!`; all DDL into `schema/{cache,user}.rs`; `Store::open` runs the schema step | `oxy-store`, `oxy-library` | 94 tests green; `cargo check --workspace --all-targets` |
| B | Repositories for favourites, tags, people (225 statements); `Library` keeps its methods for now, calling repos | `oxy-store`, `oxy-library` | tests green, no behaviour change |
| C | New `oxy-tags`; `Library` tags methods **deleted**; 19 call sites become `state.tags.*`; `TagXmpPayload` moves | + `commands/tags.rs`, `commands/folder.rs` | tests green; `oxy-tags` has no `rusqlite` dependency |
| D | `oxy-people` owns identity, review, history, references, and the detection/feature/analysis caches; the ~445 lines of person logic leave `tags.rs` and the person caches leave `cache/`; 31 call sites become `state.people.*`; the analysis methods stay because the person WIP line already uses them | + `commands/people.rs`, `commands/folder.rs`, `oxy-fs` | tests green; `oxy-people` enters the dependency graph without `rusqlite` |
| E | Cache repositories (158 statements, 3553 production lines) | `oxy-store`, `oxy-library` | **separate decision — see below** |
| P | Extract `oxy-preview` from `apps/desktop/src-tauri/src/jobs/preview.rs` | new `oxy-preview`, `apps/desktop` | its 971 test lines run without Tauri |

Stage P is **independent of A–E** and can be done at any point, since it does
not touch `oxy-library`'s internals — it belongs in this plan because it is the
same kind of move: taking behaviour out of a crate that should not own it.

### Before stage C

Three things stage B settled that change what C costs:

- **`oxy-store` must re-export `Connection` and `Transaction` first.** The gate
  for C is "`oxy-tags` has no `rusqlite` dependency" while the transaction still
  belongs to the policy layer, so the crate that opens the transaction has to
  name the type without naming `rusqlite`. Stage B did not add those re-exports
  — `oxy-library` still depends on `rusqlite` directly for its cache namespace,
  so nothing needed them yet.
- **No SQL moves in C.** Every statement in `user/tags.rs` is already a
  `repo::tags::*` function; C moves the *policy* (the method bodies, 19 call
  sites, `TagXmpPayload`) and deletes `Library`'s tags methods. The bodies are
  now short enough to read in one screen each.
- **`repo::cross::reconcile_person_source_for_{asset,subject}` stays put.** Both
  `oxy-tags` and `oxy-people` call it, which is exactly why it cannot live in
  either.

`jobs/preview.rs` is 2,847 lines (about 1,875 production, 971 test) holding
`PreviewRequest`, `PreviewIdentity`, `PreviewScheduleKey`, and `PreviewQueue` —
multi-level projections, intent scheduling, projection resource restoration.
It uses exactly **one** Tauri symbol, `use tauri::{AppHandle, Emitter}`, for
pushing events; everything else is `oxy_domain`, `oxy_library`, `oxy_media`,
`oxy_runtime`, and `state::cache::CacheManager`. Replace the handle with a small
emitter trait or a callback and the module becomes a crate.

`AGENTS.md` says to keep `apps/desktop/src-tauri` thin. At 2,847 lines this file
is the clearest violation, and the cost is concrete rather than stylistic: its
tests cannot run without compiling and linking Tauri and its two plugins.

Stage E needs its own judgement call, and stage D shrank it: the person caches
already left, so what remains is `cache/index.rs`, `cache/browsing.rs`, and
`cache/projections.rs` — 1003, and two more files whose queries *are* their
logic: paged range queries, FTS, recursive directory walks. Lifting those into
repositories means publishing many large, parameter-heavy query functions and
leaving `oxy-library` with orchestration only. That is defensible, but the gain
is smaller than for user data, where rules and statements are genuinely
different things. Recommended: re-measure now that A–D are in.

## Risks

- **The person line and the refactor want the same files.** Stage D rewrote
  `commands/people.rs` while that file was an untracked work in progress. It was
  committed with the crate it belongs to; see "Resolved by stage D" above.
- **`oxy-people` had no consumers** and 16 of its analysis methods had no
  caller. Stage D changed that deliberately by landing them into a crate that
  now owns both axes of the person domain; the tests followed, so the pipeline
  is no longer a crate that nothing exercises. What is still unwired is the
  *application*: no model-install command, no analysis-job IPC, and no candidate
  query command reach the frontend yet.
- **373 statement sites move.** Behaviour must be preserved, not improved.
  Existing inline tests (4263 lines) move with their modules and are the
  regression net; they are strongest in `tags.rs` (613) and `people.rs` (516).
- **`oxy-store` grows to roughly 4000 lines** and becomes the largest crate. If
  that becomes uncomfortable, splitting `oxy-store::repo` per domain is already
  possible without touching anything else.

## Documents to update

Done across stages A–D:

- `AGENTS.md`: "declare every table once in the module that owns it" became
  "declare every table once in `oxy-store/src/schema`, with its class"; the
  cross-namespace rule became "only `oxy-store` writes SQL; domain crates name a
  repository function and do not depend on `rusqlite`"; the thin-`src-tauri`
  rule gained the composition-root exception, and the sibling rule gained the
  dev-dependency exception that `oxy-tags` uses.
- `docs/architecture/05-data-and-state.md`: ownership and the two axes, the
  repository layer, and the sibling crates.
- `docs/architecture/06-extension-guide.md`, `docs/ARCHITECTURE.md`,
  `CONTRIBUTING.md`: crate inventory.
- `docs/PERSON_WORKFLOW.md`: the module-responsibility table now describes the
  crates that exist rather than the ones planned, and the paths it cites
  (`oxy-people/identity.rs`, `analysis_runs.rs`, `features.rs`) are the real
  ones.
