//! Native and portable decoder adapters.
//! File-format compatibility rules belong in `formats`, not platform copies.
//! Existing cross-backend fallbacks remain until the planner migration.

#[cfg(target_os = "macos")]
pub(crate) mod apple_image_io;
pub(crate) mod ffmpeg_heif;
pub(crate) mod libheif;
pub(crate) mod libraw;
#[cfg(target_os = "windows")]
pub(crate) mod windows_wic;
