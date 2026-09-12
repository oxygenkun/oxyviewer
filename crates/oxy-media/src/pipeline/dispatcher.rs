use super::{heif::artifact as heif, raw, system};
use crate::{
    MediaError, decode_control::DecodePriority, media_source::preview_result,
    pipeline::artifact::register_original_resource,
};
use oxy_domain::{AssetKind, PreviewKind, PreviewPriority, PreviewResult, RenderLevel};
use oxy_runtime::CancellationToken;
use std::{path::Path, sync::mpsc::Receiver};

pub struct AppPreview {
    pub result: PreviewResult,
    pub completions: Vec<Receiver<crate::publication::PersistenceCompletion>>,
}

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
    execute(
        path,
        cache_dir,
        kind,
        level,
        priority.into(),
        true,
        cancellation,
    )
}

/// App hot path: publishes an immutable resource before bounded cache
/// persistence. The compatibility `preview` API remains synchronous for
/// benchmarks and non-app callers.
pub fn preview_for_app(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    priority: PreviewPriority,
    kind: oxy_domain::AssetKind,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    preview_for_app_with_completion(path, cache_dir, level, priority, kind, cancellation)
        .map(|preview| preview.result)
}

pub fn preview_for_app_with_completion(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    priority: PreviewPriority,
    kind: oxy_domain::AssetKind,
    cancellation: &CancellationToken,
) -> Result<AppPreview, MediaError> {
    app_preview(path, cache_dir, level, priority, kind, true, cancellation)
}

pub fn preview_for_app_upgrade(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    priority: PreviewPriority,
    kind: oxy_domain::AssetKind,
    cancellation: &CancellationToken,
) -> Result<AppPreview, MediaError> {
    app_preview(path, cache_dir, level, priority, kind, false, cancellation)
}

fn app_preview(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    priority: PreviewPriority,
    kind: oxy_domain::AssetKind,
    allow_interim: bool,
    cancellation: &CancellationToken,
) -> Result<AppPreview, MediaError> {
    let (result, completions) = super::artifact::with_app_publication(|| {
        execute(
            path,
            cache_dir,
            kind,
            level,
            priority.into(),
            allow_interim,
            cancellation,
        )
    });
    Ok(AppPreview {
        result: result?,
        completions,
    })
}

#[allow(clippy::too_many_arguments)]
fn execute(
    path: &Path,
    cache_dir: &Path,
    kind: AssetKind,
    level: RenderLevel,
    priority: DecodePriority,
    allow_interim: bool,
    cancellation: &CancellationToken,
) -> Result<PreviewResult, MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    match (kind, level) {
        (AssetKind::Jpeg | AssetKind::Png | AssetKind::Webp, _) => register_original_resource(
            path,
            preview_result(path.to_owned(), PreviewKind::Original, level)?,
        ),
        (AssetKind::Raw, RenderLevel::Thumbnail) => {
            raw::preview_with_priority(path, cache_dir, 512, priority, allow_interim, cancellation)
        }
        (AssetKind::Raw, RenderLevel::Preview) => raw::preview_with_priority(
            path,
            cache_dir,
            4_096,
            priority,
            allow_interim,
            cancellation,
        ),
        (AssetKind::Raw, RenderLevel::Full) => {
            raw::full_with_interim(path, cache_dir, allow_interim, cancellation)
        }
        (AssetKind::Heif, RenderLevel::Thumbnail) => heif::preview(
            path,
            cache_dir,
            512,
            priority,
            true,
            true,
            allow_interim,
            false,
            cancellation,
        ),
        (AssetKind::Heif, RenderLevel::Preview) => heif::preview(
            path,
            cache_dir,
            4_096,
            priority,
            true,
            true,
            allow_interim,
            false,
            cancellation,
        ),
        (AssetKind::Heif, RenderLevel::Full) => match heif::full(path, cache_dir, cancellation) {
            result @ Ok(_) => result,
            Err(error) if !heif_full_error_allows_fallback(&error) => Err(error),
            Err(error) => {
                eprintln!(
                    "full-detail HEIF artifact attempt failed for {}: {error}",
                    path.display()
                );
                heif::preview(
                    path,
                    cache_dir,
                    8_192,
                    DecodePriority::Foreground,
                    false,
                    true,
                    allow_interim,
                    true,
                    cancellation,
                )
            }
        },
        (AssetKind::Tiff, RenderLevel::Thumbnail | RenderLevel::Preview) => {
            system::preview(path, cache_dir, 512, level, allow_interim, cancellation)
        }
        (AssetKind::Tiff, RenderLevel::Full) => {
            system::preview(path, cache_dir, 4_096, level, allow_interim, cancellation)
        }
    }
}

fn heif_full_error_allows_fallback(error: &MediaError) -> bool {
    if let MediaError::BackendAttempts { source, .. } = error {
        return heif_full_error_allows_fallback(source);
    }
    !matches!(
        error,
        MediaError::Cancelled
            | MediaError::StaleCacheGeneration
            | MediaError::StaleSourceRevision
            | MediaError::ResourceBudgetExhausted { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heif_full_generation_and_source_fences_never_fallback() {
        assert!(!heif_full_error_allows_fallback(
            &MediaError::StaleCacheGeneration
        ));
        assert!(!heif_full_error_allows_fallback(
            &MediaError::StaleSourceRevision
        ));
        assert!(!heif_full_error_allows_fallback(&MediaError::Cancelled));
        assert!(!heif_full_error_allows_fallback(
            &MediaError::ResourceBudgetExhausted {
                budget: "entry",
                current: 512,
                limit: 512,
                requested: 1,
            }
        ));
        assert!(heif_full_error_allows_fallback(
            &MediaError::NativeDecoderUnavailable
        ));
    }
}
