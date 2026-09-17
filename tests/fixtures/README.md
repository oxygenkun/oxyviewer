# Test Fixtures

Real camera files are **not tracked in git**. They are archived outside the
repository and restored into this directory (which is git-ignored) when
needed. Fixture-dependent tests skip automatically when a file is missing.

This is the only fixture root. The former `test/fixtures/media/` directory was
merged into it, so every fixture now lives directly in `tests/fixtures/`.

## Archived fixtures

| File               | Description                                                      | Size      | SHA-256                                                            |
| ------------------ | ---------------------------------------------------------------- | --------- | ------------------------------------------------------------------ |
| `DSC00449.HIF`     | Sony HEIF, 4672x7008, 10-bit, embedded 160×120 JPEG with padding | 9,490,432 | `8d226283fa6e5fdc75a7dbb70b7314c6c545fe3bd7cb74cae94e7996125aa4c4` |
| `DSC00511.HIF`     | Sony HEIF, 10-bit; the file referenced by `IDCLabelInfo.xml`     | 10,379,264 | `fc93b6a1429c4dc5d1d93581d7378a2d04909f975f8edd56e9fa3ab61b6dc989` |
| `DSC00526.HIF`     | Sony HEIF, 10-bit                                                | 6,664,192 | `e5514da5104d404f101e09ae3db67b2a96565ba56506d9ad5943779cd0247664` |
| `DSC00529.ARW`     | Sony RAW; default fixture for the ARW preview/loupe scenarios and `raw_display_bench` | 44,310,528 | `0c840ff304587ff3df365d08ac9c3399189a95da99c672ed6c1f755dfd5f5f37` |
| `DSC02948.ARW`     | Sony RAW, second sample                                          | 33,472,512 | `65102ac8e346426aa3b8a1255a8ede634773d6b174256530c400b1d9f81710f9` |
| `IDCLabelInfo.xml` | Image Capture label sidecar for `DSC00511.HIF` (`Color`, `Rate`) | 313       | `d5a9e234f5b9df52255dc4a8557a9416bcb04ff61bfbfe9dd0b9f8e554c9865c` |

### How to obtain

The fixture archive is currently kept on the maintainer's local machine
(`~/oxyviewer-fixture-archive/`). A remote download location will be
published here once the archive is uploaded. Until then, ask the maintainer
for a copy and verify the SHA-256 checksums above after download:

```bash
shasum -a 256 tests/fixtures/*
```

## Running fixture-dependent tests

Tests that need the Sony HIF fixture resolve it from
`tests/fixtures/DSC00449.HIF` by default and skip when it is absent. Point
`OXY_HIF_FIXTURE` at the file to run them against a copy stored elsewhere:

```bash
OXY_HIF_FIXTURE=/path/to/DSC00449.HIF cargo test -p oxy-media
```

`OXY_MEDIA_FIXTURE_DIR` overrides the directory scanned by the ignored
`fixture_preview_performance_budgets` test, and `OXY_RAW_FIXTURE` overrides the
file used by the ignored LibRaw smoke test:

```bash
OXY_RAW_FIXTURE=/path/to/photo.raw \
  cargo test -p oxy-media extracts_preview_from_raw_fixture -- --ignored
```

`tests/perf/scenarios.json` references these files by repo-relative path. The
runner hard-links them into its own generated directories; see
[PERF_E2E.md](../../docs/PERF_E2E.md).

Place only redistributable fixture images directly in git. The complete
matrix will cover ARW, CR2, CR3, NEF, DNG, RAF, RW2, ORF, JPEG, and 8/10-bit
HEIC, including orientation, color profile, sidecar, and metadata-conflict
cases.
