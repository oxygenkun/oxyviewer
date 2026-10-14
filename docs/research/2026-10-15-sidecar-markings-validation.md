# Sidecar markings and keyword validation — 2026-10-15

The reported grid/loupe stars became visible during investigation. Native reads
and queued projections agreed with the reference XMP values. The confirmed
keyword defect was separate: summary reads never reconciled sidecar assignments,
so tags appeared only after opening details, and already mounted filmstrip queries
could retain their earlier empty assignment result.

Summary workers now read an adjacent sidecar directly, reconcile its keyword
sources and notify the UI when assignments change. Existing cached summaries
also reconcile keywords. Discovered sidecars enter the existing metadata queue
after directory pages render, in batches of 32 at background priority with a
200 ms yield between batches. A refreshed directory snapshot readmits its paths.
Detail reads retain sidecar facts when native capture metadata fails, retaining
the native error as a document diagnostic. Pending mirror writes continue to win
over reverse imports, with an additional check inside the write transaction.

## Real fixture and measurements

macOS arm64, release build, a mounted photo volume containing 1,545 Sony HIFs
and 146 adjacent XMPs. Application state and preview caches were temporary.
The source folder was read only; all 146 XMP byte contents were compared before
and after, and no pending mirror writes remained.

All 146 summary projections reached Ready with the exact XMP distribution:

| Rating | Photos |
| --- | ---: |
| 0 | 12 |
| 1 | 5 |
| 2 | 6 |
| 3 | 43 |
| 4 | 43 |
| 5 | 37 |

13 photos imported the expected two-level Unicode keyword path. They generated
13 assignment-change events. The complete headless queue pass, including
admission, projection persistence and keyword imports, took 753.6 ms.

A seven-photo release comparison reconstructed the previous summary read by
calling `MetadataFacade::read_document_observation`; the revised path called
`read_summary_observation`. Both included observation/fingerprint work and
returned identical ratings. Three passes alternated ordering for one pass:

| Path | Pass 1 | Pass 2 | Pass 3 | Median |
| --- | ---: | ---: | ---: | ---: |
| Full document, previous summary path | 200.9 ms | 111.2 ms | 111.9 ms | 111.9 ms |
| Direct sidecar summary | 28.0 ms | 26.7 ms | 28.1 ms | 28.0 ms |

OS caches were not cleared. These are metadata component timings, not cold
folder, thumbnail throughput or first-paint qualification. The earlier debug
queue observation is excluded from this release comparison.

## Visible behavior and checks

The release WebView displayed all six rated photos in a seven-photo copied
fixture with their expected stars; the zero-rated photo displayed no star badge.
Loupe's caption and filmstrip displayed the selected photo's 3-star rating, and
the inspector and filmstrip displayed its imported Unicode keyword. The test
application instances were closed after verification.

Regression coverage includes summary reads with unreadable native image
metadata, selected detail fallback, existing cached projections importing tags,
idempotent import notifications, deferred/bounded frontend batches, pagination,
directory switches, retries, snapshot refresh and targeted query invalidation.
Frontend type checking/tests/build, workspace Rust tests and Clippy with the
contracts feature, formatting, and generated-contract drift checks passed.
The current Clippy toolchain also required replacing one existing manual modulo
alignment check with `is_multiple_of`; the condition is equivalent.

This local run does not qualify Windows/Linux behavior, cold NAS performance,
or the remaining first-paint targets in `docs/PERFORMANCE.md`.
