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
//!
//! The tag vocabulary used to live here too. It is `oxy-tags` now, and the
//! person identity is `oxy-people` — siblings over the same store — so that
//! neither domain's rules can name the other's. What remains is the favourites.

pub(crate) mod roots;
