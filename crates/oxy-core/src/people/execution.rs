//! Explicit enrollment, fenced persistence, and bounded background execution.
pub mod enrollment;
#[cfg(any(windows, target_os = "macos"))]
mod face;
#[cfg(any(windows, target_os = "macos"))]
pub mod folder;
pub mod ledger;
pub mod operations;
#[cfg(any(windows, target_os = "macos", test))]
mod persistence;
#[cfg(any(windows, target_os = "macos", test))]
mod pipeline;
#[cfg(test)]
mod tests;
