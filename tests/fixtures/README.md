# Test Fixtures

Real camera files are **not tracked in git**. They are archived outside the
repository and restored into this directory (which is git-ignored) when
needed. Fixture-dependent tests skip automatically when a file is missing.

## Archived fixtures

| File           | Description                              | Size     | SHA-256                                                            |
| -------------- | ---------------------------------------- | -------- | ------------------------------------------------------------------ |
| `DSC00449.HIF` | Sony HEIF, 4672x7008, 10-bit, no embedded thumbnail | 9,490,432 | `8d226283fa6e5fdc75a7dbb70b7314c6c545fe3bd7cb74cae94e7996125aa4c4` |

### How to obtain

The fixture archive is currently kept on the maintainer's local machine
(`~/oxyviewer-fixture-archive/`). A remote download location will be
published here once the archive is uploaded. Until then, ask the maintainer
for a copy and verify the SHA-256 checksum above after download:

```bash
shasum -a 256 tests/fixtures/DSC00449.HIF
```

## Running fixture-dependent tests

Tests that need the Sony HIF fixture resolve it from
`tests/fixtures/DSC00449.HIF` by default and skip when it is absent. Point
`OXY_HIF_FIXTURE` at the file to run them against a copy stored elsewhere:

```bash
OXY_HIF_FIXTURE=/path/to/DSC00449.HIF cargo test -p oxy-media
```

Place only redistributable fixture images directly in git. The complete
matrix will cover ARW, CR2, CR3, NEF, DNG, RAF, RW2, ORF, JPEG, and 8/10-bit
HEIC, including orientation, color profile, sidecar, and metadata-conflict
cases.

To run the LibRaw smoke test against a local or temporary RAW fixture:

```bash
OXY_RAW_FIXTURE=/path/to/photo.raw \
  cargo test -p oxy-media extracts_preview_from_raw_fixture -- --ignored
```
