//! Tensor preparation and synchronous model inference, independent of job/storage policy.
#[cfg(any(windows, target_os = "macos"))]
pub mod face;
pub mod input;
