# Active and Deferred Plans

This directory contains work that is not complete. Current contracts live in
the architecture, format, and performance documents; a task plan cannot
override them.

- [HIF foreground and background acceleration](hif-performance-plan.md): active
  follow-up work for persistent decoding and broader qualification.
- [Native image presentation](native-image-presentation-plan.md): deferred
  long-term investigation with explicit measurement gates.
- [Top search and tag filtering](top-search-tag-filter-design.md): basic hierarchical custom-tag search is implemented;
  tracks the remaining interaction refinements and full native qualification.
- [Folder drag-and-drop import](folder-drop-import-plan.md): implemented in
  0.1.3 (register, index, and status-bar feedback); remaining work is the
  real-device matrix and release-build comparison.
- [Face recognition and people](face-people-plan.md): the analyzer, persistence
  split, Host commands, review UI, and the durable non-cache person store are
  implemented with OpenCV-parity and cache-deletion tests, plus face-crop
  thumbnails, a loupe overlay for in-place correction, and tiled small-face
  detection, reversible merge/detach, identity stability across rename, move,
  and delete, threshold calibration from the user's own decisions, and
  folder-scoped analysis, source-resolution detection for raster formats, and a
  cross-language IPC contract test.

Completed implementation plans are archived in
[archive/plans](../archive/plans/README.md). Dated measurements and diagnoses
belong under [research](../research/README.md).
