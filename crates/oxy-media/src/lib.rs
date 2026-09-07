#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
compile_error!("oxy-media supports only Windows, macOS, and Linux");

mod backends;
mod cache;
mod decode_control;
mod error;
mod formats;
mod heif_service;
mod media_source;
mod pipeline;
mod presentation;

pub use cache::{CacheUsage, clear_preview_cache, preview_cache_usage, prune_preview_cache};
pub use error::MediaError;
pub use heif_service::{DEFAULT_TILE_SIZE, HeifDecodeService, HeifTile};
pub use media_source::{ImageDimensions, dimensions};
pub use pipeline::dispatcher::preview;
pub use pipeline::heif::artifact::cached_heif_full;

/// Included in persisted image projection identities so behavior-changing
/// media policy cannot reuse a ready projection that points at an older cache.
pub const PREVIEW_POLICY_VERSION: &str = "media-raw-native-v2";

/// Resolves the Sony HIF test fixture, which is archived outside git (see
/// `tests/fixtures/README.md`). `OXY_HIF_FIXTURE` overrides the default
/// `tests/fixtures/DSC00449.HIF` location. Returns `None` when the file is
/// absent so fixture-dependent tests can skip instead of failing.
#[cfg(test)]
pub(crate) fn sony_hif_fixture() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("OXY_HIF_FIXTURE").map_or_else(
        || {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/DSC00449.HIF")
        },
        std::path::PathBuf::from,
    );
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests;
