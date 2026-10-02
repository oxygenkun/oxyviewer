# Folder person clustering and review integration

Date: 2026-09-29. Windows x64. This report covers the local Release application;
other desktop platforms have not been exercised.

## Scope

Recognition now advances from persisted face detection/embedding work to a
cancellable folder grouping operation. Existing compatible completed results
can be grouped directly without installing or loading models. The result is
persisted as a cache snapshot; explicit adoption creates pending manual reviews
and a durable user-owned link to a new or existing folder person.

Policy: `adaface-complete-link-0.50-v2`. Merge strongest edges first, require
cosine similarity at least 0.50 across every pair between merging groups, and
prohibit different detections from one photo sharing a group. Singletons remain
visible in the ungrouped filter. This conservative suggestion policy has not
been calibrated as an identity acceptance rule. A group is not a confirmed
identity, and the group count is not a person count.

## Real folder evidence

Used a disposable SQLite backup of the user's analyzed NAS performance folder,
with 762 HIF photos, 803 detections and 803 current 512-D features. Original
photos and the user's production database were not modified during validation.
No decode/inference rerun was needed.

The ignored Release fixture test `clusters::tests::real_folder_cluster_fixture`
uses the production `People::cluster_folder` method:

| Result | Count |
| --- | ---: |
| Suggested groups | 87 |
| Grouped instances | 750 |
| Ungrouped instances | 53 |
| Photos with no detected face | 39 |
| Missing/stale evidence reported | 0 |
| Largest two groups | 105, 89 |

One warm-source run took 15,583 ms including fresh non-recursive NAS enumeration,
source observations, scoring, publication revalidation and SQLite persistence.
This is not a cold-network or cross-platform performance guarantee. Original
absolute source paths and image-bearing screenshots are kept outside tracked
research documents.

## Actual Release/WebView workflow

Using an isolated app-data directory and a separate WebView profile:

1. Opened the NAS folder and entered People; the 87 persisted groups appeared.
2. Selected the largest group: Grid changed from 762 to 105 photos; real HIF
   thumbnails were visible. Covers used the regular thumbnail path.
3. Named this group with a test label and clicked “加入待确认”: 105 pending
   records appeared, identity stayed unconfirmed, and the group displayed the
   manually supplied name.
4. Opened a multi-person photograph in Loupe, selected its adopted instance,
   and marked it “不属于”. The pending filter moved to 104 photos.
5. Clicked “聚类已有结果” in the real UI without installed models in the test
   app directory. It completed with 87 groups and 53 ungrouped instances;
   the user name and negative decision survived.
6. Closed and reopened the Release application with the same isolated data and
   WebView profile: the named group and 104 pending reviews were restored.
7. In the final Release build, selected the adopted group and opened a
   multi-person photograph in Loupe: exactly one “本组人脸 · 待核对” overlay
   remained visible, excluding the other person's detection.

## Automated checks

- Frontend: type check, build, 376 tests, including the adopted-group face overlay regression and the
  grouping/adoption controls.
- Rust: the full workspace suite passed (749 unit tests plus 2 doctests).
  Focused tests cover complete-link bridge prevention, same-photo exclusion,
  missing vectors, cancellation, persistence across store reopen, stale-source
  adoption rejection, superseding runs, full-snapshot filtering before paging,
  idempotent adoption, preserved explicit negatives, persisted user names and
  cache clearing. The store's all-user-table preservation fixture now includes
  the adoption table.
- Clippy with `--no-deps` passes for `oxy-people`, `oxy-store`, and the desktop
  package. The unrestricted workspace invocation is blocked by pre-existing
  `collapsible_if` errors in filesystem/metadata-parser code under Rust 1.96.
- Built with `pnpm tauri build --no-bundle`, including the repository's native
  preparation wrapper. No native source pins were changed.

Automatic cross-session identity suggestions, broad clustering accuracy
qualification, and automatic unfinished-operation recovery remain outside this
change. Manual historical linking remains available after reference confirmation.


## Subsequent global identity refactor (same date)

This stage supersedes the folder-identity/history ladder and the earlier boundary
above that cross-session suggestions were not wired. The authoritative interaction
contract is now `docs/PERSON_WORKFLOW.md`; the previous observations remain as
historical evidence for the first clustering implementation.

New persistent user records represent global identities, tuple targets/reviews,
explicit references, tag mappings, migration mappings and idempotent events. The
model publishes only rebuildable suggestions. A photo can contain multiple tuples;
each tuple can carry a face, a body, or both. Explicit history links migrate to one
global identity; equal names alone do not merge. Conflicting legacy decisions keep
an explicit exclusion, otherwise become deferred. Original records/events survive.

### Release validation on isolated copies

The original app database and NAS photos were not edited. An isolated database and
WebView profile were used, followed by one locally copied HIF for a second folder.

- Restart recovered the migrated global registry, the earlier 104 pending / one
  negative records, and later the new confirmed reference.
- Confirmed one real instance and explicitly set it as a reference. Selected-person
  retrieval on 762 ReNus photos returned one known-person group: 237 instances,
  comprising 235 pending, one confirmed and one excluded. The excluded instance
  also remained available as unknown. These are workflow counts, not accuracy scores.
- The first real retrieval exposed exact JSON equality between manual/detected
  boxes: coordinates differed by approximately 1e-17 after a round trip. Replaced
  string equality with unique numeric correspondence within 1e-9; the regression
  test and actual repeated retrieval passed.
- Auto-grouping with the same reference completed with 123 unknown groups and one
  known-person group. The “does not belong” filter continued to show its old record.
- Created a test global person, assigned one anonymous instance and confirmed it;
  transferred the remaining 88 instances as pending. The resulting group retained
  one confirmed plus 88 pending, without changing other people in the photographs.
- Opened a real HIF instance in Loupe and drew its associated body box. The UI
  displayed face and body buttons with the same instance number and still counted
  exactly one instance.
- In a second local folder without model caches, drew a body-only instance. It was
  automatically selected in its anonymous review group and could be confirmed for
  the global person created in ReNus. Renamed that person in the second folder;
  returning to ReNus showed the new name with the same 89 instances and 88 pending.
- Fixed thumbnail height inheritance and checked actual screenshots of the sidebar,
  tuple preview boxes and Loupe. Review previews are bounded to 30 rows; selecting a
  group collapses the long group list. Final confirmation navigation has a focused
  UI regression. A final native check exposed Loupe's first-row fallback during
  refetch; review now retains one active AssetSummary separately from the filtered
  list. A real Loupe component regression covers empty/intermediate/final pages
  and explicit subsequent navigation without changing the selected photo. The final
  Release repeat confirmed DSC02644.HIF remained the displayed photograph after
  direct confirmation into the existing named group.

### Final checks and boundaries

- Type checking, frontend production build and Release native build pass.
- Frontend suite: 382 tests in 70 files; Rust workspace: 757 passed, 28 ignored,
  including doctests. Ignored model/fixture tests are not claimed as executed.
- Focused tests cover cross-folder identity persistence, multi-person photographs,
  body-only instances, explicit cross-folder reference retrieval, stale source and
  reference rejection, cancellation, atomic batches, preserved exclusions,
  idempotence, legacy conflict migration, geometry invalidation, tag source
  preservation, user/cache separation and post-confirmation navigation.
- Strict scoped Clippy passes for `oxy-domain`, `oxy-store`, `oxy-people` and the desktop package. Workspace
  Clippy remains blocked by existing warnings in unchanged `oxy-metadata-parser`
  under Rust 1.96 (103 diagnostics, including collapsible-if and manual-is-multiple-of).
- Built through `pnpm tauri build --no-bundle`; no dependency pins or research
  environments were changed. Verification instances are closed after testing.

The 0.40 retrieval and 0.50 anonymous-core thresholds are exploratory review
settings. E2–E6 did not qualify for promotion; automatic clothing/body matching is
not shipped. Body-only tuples support manual work but do not produce face vectors.
Unseen-session precision/recall, automatic body inference, macOS hardware behavior
and automatic crash recovery remain outside this verification. Model extraction
was validated by the preceding media-input work; this stage reused its compatible
real features and did not claim a fresh full-folder inference benchmark.
