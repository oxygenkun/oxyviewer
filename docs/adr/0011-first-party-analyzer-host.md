# ADR 0011: First-party analyzer host and portable human face facts

Status: accepted, 2026-09-19.

The face feature separates disposable observations from human decisions. Its
first analyzer boundary must preserve that separation while sharing lifecycle,
revision and projection rules with the rest of the application.

Use a host-owned Analyzer interface with oriented RGB input and geometric output.
Run the bundled face implementation in a verified first-party subprocess. The
application owns rendering, source authorization/revision validation, persistence,
and action dispatch. Binary image/features travel outside JSON. A bounded cancel
protocol can terminate and reap an unresponsive child. This is process fault
isolation, not an OS sandbox or permission to install untrusted plugins.

Use JobRegistry ownership and bounded keyset scanning. Publish per-asset readiness
and materialized face rows atomically through resource_projections. Cross-asset
machine tables remain normalized and rebuildable. Exact clustering is capped with
an explicit failure instead of silently retaining a subset.

Persist human facts locally before projecting them into SQLite and enqueue XMP
synchronization in the same durable document update. XMP carries portable region
anchors and stable person/fact IDs, including rejection and deletion tombstones.
Retain three-way conflicts for explicit resolution; do not choose by wall-clock
last-write-wins. Keep machine biometrics out of sidecars. Cache clearing and human
fact deletion are different operations.

Host-rendered descriptors cover regions, collection selection/pagination/actions,
and settings. First-party action registries resolve only known IDs. Third-party
pack installation, independent manifest signing, WASM, iframe UI, ANN clustering,
and continuous external sidecar watching remain separate work.

See [current data contracts](../architecture/05-data-and-state.md) and the
[active face plan](../tasks/face-people-plan.md) for implementation limits.
