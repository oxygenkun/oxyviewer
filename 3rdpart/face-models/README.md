# User-managed face models

OxyViewer uses one production pair: SCRFD-10G KPS for detection and AdaFace
IR-101 for embeddings. Neither model is bundled with the application. The
People workbench downloads each file only after an explicit user action.

[`managed.json`](managed.json) is the single source of truth for model IDs,
filenames, URLs, archive metadata, byte sizes, SHA-256 digests, detector input
size, display names, and license summaries. Both the runtime installer and the
first-party analyzer worker consume this checked-in manifest.

Downloads are streamed into the application data directory, verified, and
installed atomically. Both models must be verified before analysis is enabled.
For local integration tests, set `OXY_FACE_MODEL_DIR` to a directory containing
the two filenames from `managed.json`.

Model changes invalidate only rebuildable observations, embeddings, clusters,
and candidates. Persons, tags, confirmations, and other human facts remain
portable user data.
