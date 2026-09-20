# Face test data

- `fixtures/two_people.jpg` is the redistributable real-photo integration fixture.
- `golden/align_synthetic.json` and `align_synthetic_aligned.png` pin the
  five-landmark affine alignment against OpenCV output.

Model integration tests use the SCRFD/AdaFace filenames from
`3rdpart/face-models/managed.json` and are ignored unless the user-managed files
are available under `OXY_FACE_MODEL_DIR` or `target/native/face-models`.
