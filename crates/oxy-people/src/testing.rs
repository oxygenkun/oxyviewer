//! Test-only constructors for a domain value over a throwaway store.
//!
//! The production path is the opposite: the application opens the store once
//! and hands the same `Arc<Store>` to every domain. A test has no application,
//! so it builds the store itself — and hands it over through exactly the same
//! constructor, so nothing about the domain changes shape for testing.

use crate::People;
use oxy_store::Store;
use std::path::Path;
use std::sync::Arc;

pub(crate) fn in_memory() -> People {
    People::new(Arc::new(
        Store::in_memory().expect("an in-memory store must open"),
    ))
}

pub(crate) fn open(path: &Path) -> People {
    People::new(Arc::new(
        Store::open(path).expect("the test store must open"),
    ))
}
