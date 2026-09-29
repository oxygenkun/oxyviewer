# Standalone ORT DirectML and three-stage person analysis

Date: 2026-09-29. Machine: Windows x64, Core i7-13700K, NVIDIA RTX 4090,
Intel UHD 770. This supersedes the main-application policy in the earlier
[WinML EP experiment](2026-09-29-winml-auto-ep.md); its historical measurements
remain valid only for that experiment.

## Runtime and source organization

The desktop Windows path now installs standalone Microsoft.ML.OnnxRuntime.DirectML
1.24.4 and Microsoft.AI.DirectML 1.15.4 from official NuGet packages. Package
lengths, SHA-256 and each extracted DLL digest are pinned in
`crates/oxy-people/src/environment/ort.rs`. The install directory is
`person-models/ort-directml-1.24.4-dml-1.15.4-x64`; new installations retain
both packages' licenses and third-party notices. Existing WinML directories
are left intact. The Rust wrapper uses ORT API 24 for this runtime.

- `environment/`: model catalog, verified artifact installation, runtime
  acquisition and platform session preparation.
- `inference/`: face input transformations and reusable ONNX model adapter.
- `execution/`: folder enrollment, persistent task ledger, operation lifecycle,
  bounded pipeline, face processing and transactional publication.

The main flow explicitly uses DirectML device 0 for both SCRFD and AdaFace,
with two host threads per session. On this machine device 0 is the RTX 4090.
This is a fixed device policy, not portable discrete-GPU discovery. Both
sessions must load and execute warmup successfully; errors do not silently
select CPU or WinML. DirectML memory patterns are disabled, execution is
sequential, SCRFD dimensions are frozen at 960 and AdaFace batch at 1.
WinML acquisition and EP selection are retained behind `winml-backup` and
are not called by the desktop application.

Windows pipeline version is 5, with feature space
`adaface-webface12m-f2eb07d0-media-v4-ort-dml-v1`. Changing the runtime producer
does not relabel older vectors or erase manual data.

## Pipeline behavior

Preparation, inference and persistence run concurrently, one worker each.
Two capacity-one channels provide backpressure. Preparation performs one
media decode per photo; inference detects and encodes all faces from that
same RGB image, releasing it before a result send can block. Persistence
commits detection first, then claims and commits its dependent encoder task.
Task dependency, source reobservation, generation and claim-token checks remain
at the existing database boundary.

At most three transported RGB frames of long edge 1600 coexist (about 23 MiB
in the square worst case), beyond the separate 512 MiB media preparation
admission budget and model/native allocations. Inference does not consume
preview queue workers. Cancellation stops new work and publication, drops
queued results and joins active native calls before releasing the operation.
Decode/detection failure blocks the encoder; encoding failure retains the
successfully committed detections.

## GPU execution evidence

The ignored Release test `installed_directml_runs_on_gpu` installed the new
runtime and processed real `DSC00835.jpg`: one detected face, 512-D embedding,
45.05 ms for detection plus encoding after warmup (image decoding excluded).
Both model profiles recorded two `DmlExecutionProvider` events (warmup and
real inference). These fused partition events establish actual GPU execution;
they are not an operator count. The runtime directory contains no WinML DLL.
Provider-event summaries are retained in the [raw results](2026-09-29-ort-directml-pipeline.json).

## End-to-end Release comparison

`commands::people_analysis::tests::directml_pipeline_real_folder` uses the
same `AnalysisInputService`, ORT sessions, enrollment, folder executor and
SQLite domain methods as the application. It copies 16 evenly spaced JPEGs
from dataset01 into a disposable local folder, runs three serial/pipeline
pairs with alternating order, and compares persisted detection counts.
Fixture names and SHA-256 are recorded in the raw results.

Both modes use the same warmed DirectML sessions and the same input and model
settings. Timing includes fresh folder enrollment, media preparation, GPU
inference and all SQLite commits; it excludes fixture copying and model setup
(3.708 seconds measured once). The serial reference prepares one frame and
executes detection/save/encoding/save in order, retaining that frame for both
model stages. OS file caches are not flushed; these are warm-session local
measurements, not cold-start or NAS results.

| Round | Serial, 16 photos | Pipeline, 16 photos |
| --- | ---: | ---: |
| 1 | 5523.53 ms | 4385.03 ms |
| 2 | 6223.68 ms | 4345.45 ms |
| 3 | 6011.31 ms | 4547.87 ms |
| Mean | 5919.51 ms | 4426.12 ms |

Mean elapsed time decreased **25.23%**, from 2.70 to 3.61 photos/second
(**33.75% higher throughput**). Every run persisted 32 successful steps,
zero failed steps and the same per-photo detection counts (16 faces total).
This comparison verifies detection counts and successful feature commits;
it does not assert bitwise embedding equality or model identity accuracy.

To reproduce, build the ignored test with `cargo test -p oxyviewer --release
directml_pipeline_real_folder --no-run`, set `OXY_TEST_MODEL_ROOT` to the
installed person-models directory, `OXY_BENCH_IMAGE_DIR` to dataset01/images,
and `OXY_BENCH_OUTPUT` to a disposable JSON path, then run the produced test
binary with `directml_pipeline_real_folder --ignored --nocapture`.
GPU profiling test additionally requires `OXY_TEST_FACE_IMAGE` and
`OXY_TEST_ORT_PROFILE_DIR`.

## Verification and remaining limits

- `cargo test --workspace`: passed, including bounded overlap/backpressure,
  cancellation, panic exit, failed input, source change and stale-generation
  publication tests. Real-model tests are ignored by default and were run
  explicitly as described above.
- `cargo fmt --all --check` and strict `oxy-people` all-target Clippy: passed.
- `pnpm check`, 372 frontend tests and production build: passed. Vite reports
  its existing large-chunk advisory.
- Strict workspace Clippy is blocked by existing Rust 1.96 lints in `oxy-fs`
  and `oxy-metadata-parser`; app-only Clippy also finds the existing nested
  conditional in `commands/folder.rs`. These unrelated files were not changed
  for lint cleanup.

This run does not validate the packaged desktop UI, concurrent Grid/Loupe
responsiveness, peak process memory/VRAM, mixed RAW/HIF folder throughput,
macOS or another machine's DirectML device ordering. Earlier media-format
qualification remains separate evidence; this JPEG measurement does not
replace those release gates.
