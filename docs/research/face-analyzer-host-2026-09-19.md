# Face analyzer host qualification, 2026-09-19

Environment: Apple M1, macOS 26.6.2, local disk, Release Tauri app bundle with
bundled `oxy-face-worker` and the pinned YuNet/SFace models. This is local macOS
qualification, not a Windows/Linux or NAS result.

## Automated correctness

`cargo test --workspace`: 830 passed, 19 ignored (fixture/manual scenarios).
The final embedding-fingerprint guard was then rechecked in the affected library,
userdata and desktop suites. `cargo fmt --all --check`, workspace Clippy with
`-D warnings`, `pnpm check`, `pnpm build`, and all 373 frontend tests passed.
The frontend build retains its existing chunk-size/dynamic-import warnings.


- Full workspace tests cover fingerprint changes (including max faces and
  one-bit float changes), model reference parity, cooperative clustering,
  registered-root/symlink rejection, canonical source identity, stale projection
  admission, atomic derived writes and the 100,000-target keyset/generation test.
- Real subprocess/builtin outputs agree for the 800×470 two-person JPEG and an
  empty image. Active inference cancellation, crashed child and a hung child's
  bounded termination/reaping are tested. Binary/hash/API/path-escape and framed
  payload validation are covered.
- XMP tests preserve unrelated namespaces and reject unsupported data; durable
  three-way conflicts/tombstones survive reload. A desktop service test recovers
  annotations with neither the original people document nor SQLite, and another
  binds imported facts to existing detections without inference. File copies
  retain sidecar fact IDs; intentional file deletion stops pending writes.
- Frontend tests cover descriptor source/action allowlists, batch selection,
  existing overlay/crop behavior, and displaying both sides before resolving an
  XMP conflict. Browser demo was inspected at 1280×~580: parameters, review rows
  and batch not-face operation work without runtime errors. Demo image slots are
  placeholders; this browser check is not a native rendering test.

## Concurrent face analysis and loupe

Command:

```sh
node tests/perf/perf-e2e.mjs --scenario faces-browsing --runs 3 \
  --app target/release/bundle/macos/OxyViewer.app/Contents/MacOS/oxyviewer
```

The isolated runner indexes 64 paths linked to the repository's real two-person
JPEG, begins actual subprocess analysis, waits for the first analyzed asset,
then selects eight different loupe photos while analysis is still active. Each
run starts with a cold isolated cache; normal thumbnail warming runs during
setup. These are concurrent-browsing measurements, **not eight independent cold
source decodes**. Polling checks at 50 ms intervals add up to 50 ms of observation
latency. The probe fails above the 800 ms cold-preview guardrail or if analysis
has already ended, and verifies cancellation within five seconds.

| JPEG run | Loupe median | Loupe maximum | Cancel completion |
| --- | ---: | ---: | ---: |
| 1 | 56.5 ms | 69 ms | 54 ms |
| 2 | 57 ms | 65 ms | 53 ms |
| 3 | 56.5 ms | 66 ms | 53 ms |

All three runs completed. The model subprocess was not left running after the
runner closed its applications. Reports remain in ignored `tests/perf/.reports/`.
The 800×470 JPEG alone does not qualify full camera resolution; the additional
Sony HIF run is recorded below.

## Full-resolution Sony HIF

The same `faces-browsing` probe ran against 32 generated paths linked to
`tests/fixtures/DSC00449.HIF` (7008×4672, 9.1 MiB). This uses the final Release
bundle and a cold isolated cache for each run. The analysis itself requests Full;
loupe timing is the first usable painted image, not full-resolution completion.
Normal background preview warming remains enabled, as in the JPEG test.

| HIF run | Loupe median | Loupe maximum | Cancel completion |
| --- | ---: | ---: | ---: |
| 1 | 59.5 ms | 78 ms | 52 ms |
| 2 | 59 ms | 72 ms | 54 ms |
| 3 | 59 ms | 69 ms | 54 ms |

All three runs passed and their model workers were reaped. Reports use the
`faces-browsing-hif.runN.json` prefix under `tests/perf/.reports/`. The external
scenario configuration used the following entry (the fixture is local and not
required by the default repository scenario):

```json
{
  "scenarios": {
    "faces-browsing-hif": {
      "fixture": { "type": "file", "path": "tests/fixtures/DSC00449.HIF", "count": 32 },
      "resourceStress": "faces-browsing",
      "awaitMarks": ["resource:stress-complete"],
      "coldCache": true,
      "runs": 3,
      "timeoutMs": 180000,
      "budgets": {}
    }
  }
}
```

Pass this file through `--config <path> --scenario faces-browsing-hif` with the
same `--app` argument above. Repeated paths exercise queue contention and real
media/model work; they do not represent a diverse recognition-accuracy corpus.

## Scope limits

No new 100k-directory first-paint qualification is claimed; the existing
300 ms miss in PERFORMANCE.md remains. The 100k test here qualifies bounded
SQL target enumeration and cursor semantics only. No ANN, untrusted plugin
installation, independent signing or OS sandbox is implemented. External XMP
imports follow index generations and local changes rather than a continuous
watcher. In-place content edits preserving identity, size and mtime are outside
the canonical stat-based revision contract. Future-version and offline sidecars
remain pending instead of being overwritten. Global people without photo
assignments and the undo journal remain local user data.
