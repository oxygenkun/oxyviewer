# Face Model Pack

Pinned ONNX models for the `oxy-faces` analyzer. They are downloaded inputs, not
compiled artifacts, so they live here as content-addressed manifests instead of
submodules.

| Model | Version | License | Size | SHA-256 |
| --- | --- | --- | --- | --- |
| YuNet face detection | 2023mar | MIT | 232,589 B | `8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4` |
| SFace face recognition | 2021dec | Apache-2.0 | 38,696,353 B | `0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79` |

Both come from [OpenCV Zoo](https://github.com/opencv/opencv_zoo) and are pinned
to the commit recorded in `source.json`; the SHAs were verified against the
Git-LFS object ids in that repository. Upstream `LICENSE` files sit beside each
model in the pinned trees:

- <https://github.com/opencv/opencv_zoo/tree/f12e12798e8314f7c074a6656816c048dcc95b7a/models/face_detection_yunet>
- <https://github.com/opencv/opencv_zoo/tree/ba91a3b91d00d76e86540d4013f944bd6b514e39/models/face_recognition_sface>

## Prepare

```bash
pnpm faces:prepare
```

Downloads and verifies both models and their upstream license texts into
`target/native/face-models`, then stages copies (plus a generated `NOTICE.md`)
under `apps/desktop/src-tauri/resources/face-models/` for packaging. Re-running
is safe: a file that already matches its SHA-256 is not fetched again.

The step needs network access and is not part of `pnpm native:prepare`, because
the face analyzer is optional and must not gate a media build. `pnpm tauri
build` runs it automatically before bundling, so a release package contains the
models under its resource directory; a package built without them reports the
people feature as unavailable instead of failing.

## Runtime resolution

`oxy-faces` never downloads anything. The Host resolves the two model paths and
passes them to `FaceAnalyzer::load`. Tests resolve the directory from
`OXY_FACE_MODEL_DIR` and otherwise fall back to `target/native/face-models`;
model-dependent tests skip when it is absent.

## Updating a model

A model change is a detection/embedding change, never a user-data change:

1. Bump `version`, `url`, `sha256`, `sizeBytes`, and `license` in `source.json`.
2. Update the table above.
3. Run `pnpm faces:prepare`, then `cargo test -p oxy-faces`.
4. Regenerate the parity goldens under `crates/oxy-faces/tests/data/golden` with
   `crates/oxy-faces/tests/data/generate_reference.py` and commit them.

Existing persons, linked tags, and confirmations must survive the update; only
machine observations, embeddings, clusters, and candidates are invalidated.
