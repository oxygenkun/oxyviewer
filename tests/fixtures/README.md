# Test Fixtures

Place only redistributable fixture images in this directory. The complete
matrix will cover ARW, CR2, CR3, NEF, DNG, RAF, RW2, ORF, JPEG, and 8/10-bit
HEIC, including orientation, color profile, sidecar, and metadata-conflict
cases.

To run the LibRaw smoke test against a local or temporary RAW fixture:

```bash
OXY_RAW_FIXTURE=/path/to/photo.raw \
  cargo test -p oxy-media extracts_preview_from_raw_fixture -- --ignored
```
