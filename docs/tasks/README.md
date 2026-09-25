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
- [Store, repository, and domain split](store-repository-split-plan.md): moves
  every table definition and SQL statement into `oxy-store`, splits tags and
  people into their own crates, and leaves the cache repositories in
  `oxy-library`. Stages 0-D are done; E and P remain, and the cache repositories
  are deliberately left as a later decision.

Completed implementation plans are archived in
[archive/plans](../archive/plans/README.md). Dated measurements and diagnoses
belong under [research](../research/README.md).
