//! User-owned facts.
//!
//! Everything in this namespace was typed, named, confirmed, or assigned by
//! the user. It is the reason the library exists, so it is never dropped as a
//! side effect of cache maintenance, cache schema migration, or clearing
//! previews. Changing these rows requires an explicit user action and, for
//! destructive ones, an explicit confirmation.
//!
//! The tables declare themselves `user` in `oxy_store::schema::user`, and that
//! word is the classification: no cache clear can reach a row of one, because
//! the declaration that creates it also decides who may delete from it.

pub(crate) mod people;
pub(crate) mod roots;
pub(crate) mod tags;
