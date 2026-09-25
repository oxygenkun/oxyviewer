//! Every SQL statement the rest of the workspace runs.
//!
//! A repository function owns one query. It takes the connection — or the
//! transaction, because `Transaction` dereferences to `Connection` — and
//! returns either an `oxy-domain` value or, where the result is a query shape
//! rather than a domain object, a small row type declared next to the query.
//!
//! Repositories do not open transactions and they decide nothing. "These four
//! writes are one atomic act", "a tag cannot be moved below itself", "a
//! changed region needs a new review" are rules, and they live in the crate
//! that owns the domain. What lives here is the statement, so the SQL of one
//! domain has exactly one place to be read and changed.
//!
//! The modules follow table ownership, not call order:
//!
//! - [`library`] — the explicit roots the user added.
//! - [`tags`] — the vocabulary, the assignments, and the XMP mirror.
//! - [`people`] — identity, instances, reviews, references, history, and the
//!   person caches, which are declared `cache` but belong to this domain.
//! - [`cross`] — the few functions that read or write two domains at once, and
//!   the two acts — a file that moved, a file that is gone — that no single
//!   domain can perform. They cannot live in a domain crate without making two
//!   siblings depend on each other, which is the boundary the split exists to
//!   hold.

pub mod cross;
pub mod library;
pub mod people;
pub mod tags;
