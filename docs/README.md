# OxyViewer Documentation

This directory separates current rules and operating guides from plans and
historical evidence. Start with the smallest document that matches the change;
research reports explain measured results but do not override current contracts.

## Read by task

| Change | Read first | Then use |
| --- | --- | --- |
| Cross-layer ownership, IPC, or a new subsystem | [Architecture overview](ARCHITECTURE.md) | [Architecture topics](architecture/README.md) and relevant [ADRs](adr/README.md) |
| Browsing, preview cache, queues, cancellation, or diagnostics | [Performance invariants](PERFORMANCE_INVARIANTS.md) | [Performance budgets](PERFORMANCE.md), [E2E harness](PERF_E2E.md), and [queue observability](DEBUG_QUEUE_OBSERVABILITY.md) |
| Media format, decoder, metadata, or platform fallback | [Format support](FORMAT_SUPPORT.md) | [Decode check sheet](MEDIA_DECODE_CHECK_SHEET.md), architecture topics, and platform guides |
| FFmpeg source, sidecars, or installers | [FFmpeg packaging](FFMPEG_PACKAGING.md) | Relevant reports under [research](research/README.md) |
| Rust implementation or lint policy | [Rust style](RUST_STYLE.md) | [CONTRIBUTING.md](../CONTRIBUTING.md) for commands and workflow |
| Product status or future work | [Roadmap](ROADMAP.md) | [Active task plans](tasks/README.md) |
| Native dependency source or licensing | [Native component inventory](../3rdpart/README.md) | [Third-party notices](../THIRD_PARTY_NOTICES.md) |

## Document roles

- `ARCHITECTURE.md`, `architecture/`, and `adr/` describe current design and
  durable decisions.
- `PERFORMANCE_INVARIANTS.md`, `FORMAT_SUPPORT.md`, and the focused platform
  guides are current behavioral contracts.
- `PERFORMANCE.md` defines budgets and summarizes current qualification status;
  dated measurements and experiments belong under `research/`.
- `tasks/` contains only active or explicitly deferred plans. Completed plans
  are retained under `archive/plans/` as historical implementation records.
- Build setup, commands, verification, and contribution workflow belong in
  [CONTRIBUTING.md](../CONTRIBUTING.md), not in project overview documents.

When documents disagree, verify the implementation and update the current
contract. Keep old measurements dated and scoped instead of presenting them as
current cross-platform behavior.
