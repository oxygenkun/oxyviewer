# ADR 0001: No Blocking Import

## Decision

Opening a folder must never require an import or complete recursive index.
OxyViewer returns basic file summaries page by page, then schedules thumbnails,
metadata, and optional library indexing in the background.

Only roots explicitly added by the user are eligible for persistent library
indexing. Cache data is rebuildable and may be deleted without affecting source
photos.

