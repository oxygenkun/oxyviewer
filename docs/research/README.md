# Research and Verification Records

This directory contains dated experiments, measurements, diagnoses, and local
qualification evidence. Reports state their fixture, platform, cache, and
measurement limits; they do not define current product behavior by themselves.

- [2026-09-29: analysis input bottleneck and precision-preserving SIMD resampling](2026-09-29-analysis-input-optimization.md)
  — stage attribution, rejected integer-rounding trial, pixel/embedding parity and Release comparison.
- [2026-09-29: standalone ORT DirectML and bounded analysis pipeline](2026-09-29-ort-directml-pipeline.md)
  — current Windows policy, GPU profiling, real decode/infer/SQLite Release comparison.
- [2026-09-29: Windows ML CPU / RTX / DirectML / OpenVINO speed comparison](2026-09-29-winml-backend-speed.md)
  — 16 real photos, repeated Release measurements, startup costs and Intel GPU feature divergence.
- [2026-09-29: Windows ML automatic EP acquisition and RTX inference](2026-09-29-winml-auto-ep.md)
  — vendor priority, real JPEG detection/embedding, CPU parity and DirectML fallback.
- [2026-09-28: shared analysis/media input contract and real format measurements](2026-09-28-analysis-media-input.md)
- [2026-09-28: third-party AdaFace ONNX source parity](2026-09-28-adaface-third-party-onnx.md)
  — WebFace12M versus WebFace4M correction, 204 identical named parameters and 32-crop author-weight parity.

- [2026-09-28: model downloads and folder analysis in the desktop UI](2026-09-28-person-download-folder-ui.md)
  — real downloads, cancellation, 60 MP JPEG detection/embeddings, manual-negative preservation and restart checks.

- [2026-09-24: official AdaFace source and ONNX export](2026-09-24-official-adaface-source.md)
  — pinned upstream checkpoint, verified export, feature-space mismatch and default-install gaps.

- [2026-09-24: Rust, Windows ML and tract ONNX backends](2026-09-24-rust-windows-ml-onnx.md)
  — self-contained runtime and EP constraints; local SCRFD/AdaFace tract CPU probe.

- [2026-09-24: cross-platform ORT backend design](2026-09-24-cross-platform-ort-backends.md)
  — shared SCRFD/AdaFace contracts, Windows and macOS provider boundaries and Mac validation gaps.

- [2026-09-23: manual people workflow stage one](2026-09-23-person-manual-workflow.md)
  — model-independent boxes, per-instance review, paged person filtering and persistence.

- [2026-09-23: one versus three onsite reference photos](2026-09-23-fewshot-onsite-filter.md)
  — matched candidate pools, quality/pose reference selection and score aggregation.

- [2026-09-23: one-shot onsite face filtering](2026-09-23-oneshot-onsite-filter.md)
  — one selected photo as reference, face-only retrieval, pose buckets,
  threshold transfer and missing-feature diagnostics.

- [2026-09-23: anonymous E3–E6 structural validation](2026-09-23-anonymous-e3-e6-validation.md)
  — prototype, core merge, neighbor centralization and sparse graph ablations;
  main test and whole-burst back-view holdouts.

- [2026-09-23: anonymous E2 quality ablation and aggregation pipeline](2026-09-23-anonymous-e2-pipeline.md)
  — frozen-feature quality-score tests, back-view holdouts, and the E3–E6
  experiment sequence and safety gates.

- [2026-09-23: anonymous E0–E1, onsite and post-production](2026-09-23-anonymous-e0-e1.md)
  — latest reviewed snapshot; automatic/manual boxes, LVFace-T/S CPU/CUDA,
  task-specific clustering, back-view holdouts and reproducible audit artifacts.

- [2026-09-23: dataset01 reviewed minidataset E0 pilot](2026-09-23-reviewed-minidataset-e0.md)
  — original metadata copied into JPEG proxies; frozen reviewed subset,
  capture-time splits, retrieval/clustering results and cold/warm reproducibility.

For current requirements, return to the [documentation index](../README.md),
[performance budgets](../PERFORMANCE.md), or
[performance invariants](../PERFORMANCE_INVARIANTS.md).
