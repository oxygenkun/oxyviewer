//! Rebuildable derived state.
//!
//! Everything in this namespace can be deleted and regenerated from the
//! original photos plus the user-owned facts in [`crate::user`]. Losing any of
//! it costs re-scanning, re-decoding, or re-running a model pass; it never
//! costs a name, a review decision, or a tag the user assigned.
//!
//! The tables, their class, and the entry point that empties them live in
//! `oxy_store::schema::cache`, next to the DDL, because that is where the
//! decision "this table is derived" is written down. What stays here is the
//! behaviour over that state. Nothing here may delete from a [`crate::user`]
//! table, and no module outside this one may empty a derived table by pattern.

pub(crate) mod browsing;
pub(crate) mod index;
pub(crate) mod projections;

use crate::{Library, LibraryError};

impl Library {
    /// Clears derived state only. User identity, reviews, tags, and explicit
    /// roots survive by construction; the audit in `oxy_store::schema` fails if
    /// one of them loses a row.
    ///
    /// In-memory snapshots are dropped with the rows they mirror, so a slot
    /// cannot answer from data that no longer exists on disk.
    pub fn clear_rebuildable_cache(&self) -> Result<(), LibraryError> {
        self.store.clear_rebuildable_cache()?;
        self.directory_snapshots.invalidate_all();
        Ok(())
    }
}
