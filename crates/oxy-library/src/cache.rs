//! Rebuildable derived state.
//!
//! Everything in this namespace can be deleted and regenerated from the
//! original photos plus the user-owned facts in [`crate::user`]. Losing any of
//! it costs re-scanning, re-decoding, or re-running a model pass; it never
//! costs a name, a review decision, or a tag the user assigned.
//!
//! Nothing here may delete from a [`crate::user`] table, and no cache-clearing
//! entry point outside this module may delete rows by table name pattern.
//! Each submodule empties the tables it defines; [`crate::schema`] audits the
//! result rather than driving it.

pub(crate) mod analysis;
pub(crate) mod browsing;
pub(crate) mod detections;
pub(crate) mod features;
pub(crate) mod index;
pub(crate) mod projections;

use crate::{Library, LibraryError};
use rusqlite::Connection;

/// Every rebuildable table, collected from the modules that create them.
///
/// There is no registry to update: a module's tables are listed in the module
/// itself, next to the DDL that creates them and the `clear()` that empties
/// them.
pub(crate) fn tables() -> impl Iterator<Item = &'static str> {
    index::TABLES
        .iter()
        .copied()
        .chain(projections::TABLES.iter().copied())
        .chain(browsing::TABLES.iter().copied())
        .chain(detections::TABLES.iter().copied())
        .chain(features::TABLES.iter().copied())
        .chain(analysis::TABLES.iter().copied())
}

/// Cache tables a clear leaves their allocator or format marker in place.
///
/// Resetting a revision counter would let a worker that started before the
/// clear publish an older result with a higher revision and win over
/// everything written after. Clearing a schema marker would only force a
/// needless rebuild of an otherwise valid cache.
pub(crate) fn preserved_on_clear() -> impl Iterator<Item = &'static str> {
    index::PRESERVED
        .iter()
        .copied()
        .chain(projections::PRESERVED.iter().copied())
        .chain(browsing::PRESERVED.iter().copied())
        .chain(detections::PRESERVED.iter().copied())
        .chain(features::PRESERVED.iter().copied())
        .chain(analysis::PRESERVED.iter().copied())
}

/// Creates or migrates every rebuildable table. Called after the user-owned
/// schema so user facts are never the thing that gets rebuilt on a version
/// bump.
pub(super) fn ensure_schema(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    browsing::ensure_schema(connection)?;
    index::ensure_schema(connection)?;
    projections::ensure_schema(connection)?;
    detections::ensure_schema(connection)?;
    features::ensure_schema(connection)?;
    analysis::ensure_schema(connection)
}

/// Drops the contents of every rebuildable table that currently exists.
///
/// Each module clears the tables it defines, so the decision "this table is
/// cache" is made once, next to the DDL, instead of in a second list that can
/// drift. [`crate::schema`] no longer drives this; it audits it. The tests in
/// [`crate::schema`] fail if a rebuildable table survives or a user-owned one
/// loses a row.
pub(crate) fn clear(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    let transaction = connection.transaction()?;
    index::clear(&transaction)?;
    projections::clear(&transaction)?;
    browsing::clear(&transaction)?;
    detections::clear(&transaction)?;
    features::clear(&transaction)?;
    analysis::clear(&transaction)?;
    transaction.commit()
}

impl Library {
    /// Clears derived state only. User identity, reviews, tags, and explicit
    /// roots survive by construction; see the tests in [`crate::schema`].
    ///
    /// In-memory snapshots are dropped with the rows they mirror, so a slot
    /// cannot answer from data that no longer exists on disk.
    pub fn clear_rebuildable_cache(&self) -> Result<(), LibraryError> {
        let mut connection = self.connection.lock();
        clear(&mut connection)?;
        self.directory_snapshots.invalidate_all();
        Ok(())
    }
}
