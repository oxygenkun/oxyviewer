//! User-owned facts.
//!
//! Everything in this namespace was typed, named, confirmed, or assigned by
//! the user. It is the reason the library exists, so it is never dropped as a
//! side effect of cache maintenance, cache schema migration, or clearing
//! previews. Changing these rows requires an explicit user action and, for
//! destructive ones, an explicit confirmation.
//!
//! Being in this namespace is the classification. A table listed by a module
//! here is user-owned; nothing can mark it rebuildable without moving it.

pub(crate) mod people;
pub(crate) mod roots;
pub(crate) mod tags;

use rusqlite::Connection;

/// Every user-owned table, collected from the modules that create them.
pub(crate) fn tables() -> impl Iterator<Item = &'static str> {
    roots::TABLES
        .iter()
        .copied()
        .chain(tags::TABLES.iter().copied())
        .chain(people::TABLES.iter().copied())
}

/// Creates or migrates every user-owned table. Runs before the cache schema so
/// a cache migration can never be the first thing to define user storage.
pub(crate) fn ensure_schema(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    roots::ensure_schema(connection)?;
    tags::ensure_schema(connection)?;
    tags::ensure_source_schema(connection)?;
    people::ensure_schema(connection)
}
