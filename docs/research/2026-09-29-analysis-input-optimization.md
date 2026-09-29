# Person analysis: input bottleneck attribution and optimization

Date: 2026-09-29. Windows x64, Core i7-13700K, RTX 4090. Same 16 JPEG
fixtures and standalone ORT DirectML models as the
[pipeline benchmark](2026-09-29-ort-directml-pipeline.md).
[Raw results, fixture hashes and parity measurements](2026-09-29-analysis-input-optimization.json).

## Attribution

Opt-in `OXY_ANALYSIS_TIMING=1` measures preparation/claim, inference, persistence,
and channel waits in `execution/pipeline.rs`. JPEG preparation separately
measures metadata, IDCT decoding, resampling and orientation. Timings are
emitted only when requested; no filesystem paths or image bytes are logged.
The final table uses only the three pipeline rounds, 48 image observations.
Preparation excludes the final empty-queue probe. Means are not stage sums:
the three workers overlap.

| Per-image stage | Before | Accepted optimization |
| --- | ---: | ---: |
| Input preparation including claim | 270.58 ms | 234.93 ms |
| — JPEG metadata | 1.11 ms | 1.17 ms |
| — libjpeg scaled decode | 166.63 ms | 166.87 ms |
| — final triangle resize | 93.15 ms | 56.44 ms |
| — orientation (these fixtures need no rotation) | <0.01 ms | <0.01 ms |
| Model detection and encoding | 47.22 ms | 48.15 ms |
| Fenced SQLite publication | 7.94 ms | 7.39 ms |

The remaining approximately 10 ms in preparation includes task claim, file
reads/probes and source checks; it was not separately attributed. The accepted
run's preparation send wait averages 0.0063 ms and inference send wait 0.0058 ms.
Inference receive waits total about 3.034 seconds per batch, including startup
and final channel closure. This is direct evidence that inference waits for
input, rather than persistence backing up the pipeline.

Before optimization, 16 × 270.58 ms = 4.329 seconds of preparation explains
nearly all of the 4.397-second batch. Model work totals only 0.756 seconds
and overlaps preparation. These CPU-side call durations are not hardware GPU
utilization counters. Model initialization is outside the batch timer.

## Implementation and numeric gate

`pipeline/resize.rs` uses pinned
[`fast_image_resize` 5.2.2](https://docs.rs/crate/fast_image_resize/5.2.2)
with no Rayon pool. JPEG pixel preparation above the 512-edge thumbnail tier
uses triangle convolution on RGB16 values: each source channel is shifted
left eight bits, preserving eight fractional bits through intermediate passes;
the result rounds once back to RGB8. Aspect-ratio rounding, encoded color space,
orientation handling, 1600-edge input and model preprocessing remain the same.
Thumbnail normalization keeps the prior resampler.

The preparation gate still admits one worker under 512 MiB. It reserves the
larger of decode working space and the resampling phase: three RGB16 buffers
(18 bytes per IDCT output pixel), encoded input and existing scratch allowance.
The RGB8 source is released after conversion, and the RGB16 source/scratch
before final output conversion. No new source cache or decoder threads were
added. Source reobservation, cancellation and task publication fences remain.

Three alternatives were tested, without relaxing the predefined model gate
(same face count, box coordinates within 2 pixels, feature cosine >0.995):

| Variant | Evidence | Decision |
| --- | --- | --- |
| RGB8 SIMD | About 13 ms resize; one feature cosine fell to 0.970948 despite maximum channel difference 1/255 | Rejected |
| RGB32F SIMD | Minimum feature cosine 0.999995; full batch 4.005 s | Correct but slower than accepted variant |
| RGB16 with eight fractional bits | Minimum feature cosine 0.999887; full batch 3.826 s | Accepted |

Accepted variant, 16 real photos / 16 faces:

- Same per-photo detection counts and 32 successful persisted steps each run.
- Maximum per-channel difference 1/255; mean absolute channel difference
  0.001760/255 across photos.
- Maximum detection-box coordinate difference 0.07984 pixels at 1600 edge.
- Minimum full-path AdaFace feature cosine 0.999887, using each version's
  independently detected landmarks and aligned crop.

Synthetic tests additionally cover awkward aspect ratios, near-identity
scaling, narrow images, unchanged small inputs, and mean rounding error below
0.02/255, guarding against accidentally reverting to RGB8 intermediate rounding.
These are numerical compatibility checks, not identity-quality calibration.

Windows manifest is v6, input schema `media-oriented-rgb-full-frame-1600-v5-simd-triangle`,
feature space `adaface-webface12m-f2eb07d0-media-v5-ort-dml-v1`. Old vectors
and manual facts remain; the new producer never relabels old features.

## Final end-to-end result

After builds/tests finished, retained old and accepted Release binaries ran
the same real-media/SQLite integration benchmark. Each executable alternates
serial and pipeline ordering over three rounds. Only pipeline rounds are
compared here; OS cache is not flushed, models are warmed, and diagnostics
are enabled. Fixture copies and model initialization are excluded.

| 16-photo batch, three-round mean | Before | After |
| --- | ---: | ---: |
| Wall time | 4396.59 ms | 3826.23 ms |
| Throughput | 3.64 photos/s | 4.18 photos/s |

Elapsed time decreased **12.97%**; throughput increased **14.91%**. Resize
itself decreased **39.41%**. The larger gain from the rejected RGB8 trial is
not a delivered product improvement. The remaining dominant cost is JPEG
decoding (~167 ms/photo); this patch does not claim to saturate the GPU.

## Verification and reproduction

- `cargo test --workspace`: 747 passed, 27 ignored (including opt-in fixtures).
- Explicit Release `real_jpeg_resize_parity` and
  `resized_inputs_preserve_model_outputs`: passed for the 16 fixtures.
- Explicit Release `directml_pipeline_real_folder`: passed for both versions.
- `cargo fmt --all --check`: passed. Strict workspace Clippy remains blocked
  by pre-existing lints in `oxy-fs` / `oxy-metadata-parser`; focused media Clippy
  also reports five existing lints in FFmpeg/libheif/decode-control/Sony code,
  none in the new resampler or timing implementation.

Set `OXY_BENCH_IMAGE_DIR`, `OXY_TEST_MODEL_ROOT`, `OXY_BENCH_OUTPUT` as documented
in the pipeline report and enable `OXY_ANALYSIS_TIMING=1`. For parity, set
`OXY_RESIZE_PARITY_DIR` to a disposable output directory; first run the
`oxy-media` ignored Release test `real_jpeg_resize_parity`, then `oxy-people`
test `resized_inputs_preserve_model_outputs`. The first writes paired old/new
PNG inputs plus pixel metrics; the second writes `models.json`.

No new packaged-UI, foreground scrolling, peak RSS/VRAM, NAS, mixed-format
throughput, macOS or unseen-scene recognition-quality claims are made by this
JPEG benchmark. Existing cache and UI paths were not used as substitutes for
the measured original-file input path.
