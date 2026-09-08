use super::{heif::artifact as heif, raw, system};
use crate::{MediaError, decode_control::DecodePriority, media_source::preview_result};
use oxy_domain::{AssetKind, PreviewKind, PreviewPriority, PreviewResult, RenderLevel};
use oxy_runtime::CancellationToken;
use std::path::Path;

/// Unified preview dispatcher. Callers request a semantic [`RenderLevel`];
/// this module alone chooses the platform/format-specific decoder and size.
pub fn preview(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    priority: PreviewPriority,
    kind: oxy_domain::AssetKind,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    execute(path, cache_dir, kind, level, priority.into(), cancellation)
}

fn execute(
    path: &Path,
    cache_dir: &Path,
    kind: AssetKind,
    level: RenderLevel,
    priority: DecodePriority,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    match (kind, level) {
        (AssetKind::Jpeg | AssetKind::Png | AssetKind::Webp, _) => {
            preview_result(path.to_owned(), PreviewKind::Original, level)
        }
        (AssetKind::Raw, RenderLevel::Thumbnail) => {
            raw::preview_with_priority(path, cache_dir, 512, priority, cancellation)
        }
        (AssetKind::Raw, RenderLevel::Preview) => {
            raw::preview_with_priority(path, cache_dir, 4_096, priority, cancellation)
        }
        (AssetKind::Raw, RenderLevel::Full) => raw::full(path, cache_dir, cancellation),
        (AssetKind::Heif, RenderLevel::Thumbnail) => {
            heif::preview(path, cache_dir, 512, priority, true, true, cancellation)
        }
        (AssetKind::Heif, RenderLevel::Preview) => {
            heif::preview(path, cache_dir, 4_096, priority, true, true, cancellation)
        }
        (AssetKind::Heif, RenderLevel::Full) => match heif::full(path, cache_dir, cancellation) {
            result @ Ok(_) | result @ Err(MediaError::Cancelled) => result,
            Err(error) => {
                eprintln!(
                    "full-detail HEIF decode failed for {}: {error}",
                    path.display()
                );
                heif::preview(
                    path,
                    cache_dir,
                    8_192,
                    DecodePriority::Foreground,
                    false,
                    true,
                    cancellation,
                )
            }
        },
        (AssetKind::Tiff, RenderLevel::Thumbnail | RenderLevel::Preview) => {
            system::preview(path, cache_dir, 512, level)
        }
        (AssetKind::Tiff, RenderLevel::Full) => system::preview(path, cache_dir, 4_096, level),
    }
}
