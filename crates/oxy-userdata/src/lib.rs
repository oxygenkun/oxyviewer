//! Durable, cache-independent storage for OxyViewer's user data.
//!
//! Everything the application can rebuild — the index, previews, face
//! observations, clusters, the SQLite file itself — is a cache. This crate is
//! for the opposite category: facts the user produced, which no re-scan can
//! recover and which must survive a cache wipe, a crash, and a downgrade.
//!
//! The axis is *rebuildability*, not subject matter. A person confirmation and a
//! chosen cache directory are the same kind of fact; they differ only in how
//! much is at stake, so they share one mechanism instead of each hand-rolling
//! atomic writes and version checks:
//!
//! ```text
//! UserDocument + DocumentStore   mechanism: envelope, atomic replace,
//!                                persist-before-publish, refuse unknown files
//!   ├─ people::PeopleDocument    app_data_dir/people.json
//!   ├─ cache settings            app_data_dir/cache-settings.json
//!   └─ external applications     app_data_dir/external-apps.json
//! ```
//!
//! One document per file is deliberate. A single combined user-data file would
//! make one unreadable byte cost every decision at once, force one schema
//! version on unrelated features, and rewrite the entire user history on every
//! edit.
//!
//! # Direction of authority
//!
//! SQLite is a *projection* of these documents, never a peer of them. A
//! mutation is written here first and projected second, so a crash in between
//! costs a projection refresh and never a user fact. The reverse direction —
//! reading a durable document back out of the cache — happens exactly once, when
//! a store is created for a library that already had user data in SQLite.
//!
//! # What belongs here
//!
//! A document belongs in this crate when losing it is unacceptable *and*
//! re-deriving it is impossible. Machine output that merely takes time to
//! recompute — face embeddings, the tag/XMP queue, directory snapshots — stays
//! in the cache even when it feels user-facing.

mod face_sync;
pub use face_sync::merge_face_facts;
mod document;
mod people;

pub use document::{DocumentStore, StoreError, StoreOrigin, UserDocument};
pub use people::{
    MAX_UNDO_OPERATIONS, PeopleDocument, PeopleError, PersonOperation, PersonStore, STORE_VERSION,
    UndoableOperation, prune_orphaned_decisions,
};
