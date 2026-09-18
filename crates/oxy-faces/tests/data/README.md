# oxy-faces test data

## `golden/`

Reference values produced by OpenCV, not by `oxy-faces` itself, so the parity
tests in `tests/reference_parity.rs` compare two independent implementations
instead of asserting the Rust code against its own output.

| File | Covers |
| --- | --- |
| `sface_synthetic.json` | SFace embedding of the deterministic `rgb-lcg-v1` pattern (preprocessing, channel order, inference) |
| `yunet_synthetic.json` | YuNet on the same pattern: no face must be reported |
| `align_synthetic.json`, `align_synthetic_aligned.png` | 5-landmark similarity transform and the resulting 112x112 warp |
| `portrait_reference.json` | Full pipeline on the committed fixture: boxes, landmarks, scores, embeddings |

Regenerate with `generate_reference.py` (see its docstring). The script needs
`opencv-python-headless` and the models from `pnpm faces:prepare`; the tests
themselves need only the committed files plus the models.

## `fixtures/two_people.jpg`

800x470 JPEG, 74,727 bytes, SHA-256
`5f9c6b116e5772a1a2d3a692b2728da644ee80463101c52e699674d81d196fb8`.

- Source: `ageitgey/face_recognition` at commit
  `112d636609de01c8623366d056a67a8a1d798675`, `examples/two_people.jpg`
  (repository license: MIT).
- The photograph itself is a US Government work (official White House photo of
  President Barack Obama and Vice President Joe Biden), which places it in the
  public domain. It is redistributable and safe to commit.
- `generate_reference.py` downloads the source and downscales it to 800 px wide,
  so the committed bytes are reproducible from the pinned URL.

The portrait exists to prove that detection, alignment, and embedding work on a
real photograph with more than one face, which a synthetic pattern cannot show.
Synthetic-only coverage would not catch a wrong channel order or a swapped
landmark order that still produces self-consistent output.

## Adding coverage

Add another redistributable portrait to `fixtures/`, extend
`generate_reference.py`, regenerate `golden/`, and commit both. Do not add
fixtures whose license does not clearly permit redistribution; do not point a
test at a path outside this directory.
