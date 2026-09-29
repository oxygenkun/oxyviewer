//! Explicit model/runtime installation and platform session preparation.
pub mod artifacts;
pub mod catalog;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod directml;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod ort;
#[cfg(all(windows, target_arch = "x86_64", feature = "winml-backup"))]
pub mod winml;
#[cfg(all(windows, target_arch = "x86_64", feature = "winml-backup"))]
pub mod winml_runtime;
