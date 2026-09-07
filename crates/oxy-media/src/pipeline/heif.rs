//! HEIF backend policy and preview artifact execution.
//!
//! `backend` owns backend probing, ordering, fallback, and attempt diagnostics.
//! `artifact` owns cached thumbnail, preview, and full-image production.

pub(crate) mod artifact;
pub(crate) mod backend;
