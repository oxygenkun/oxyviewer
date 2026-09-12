use super::{heif, raw, system};
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
/// format pipelines own request policy, backend choices, and fallback.
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
    let result = match (kind, level) {
        (AssetKind::Jpeg, RenderLevel::Thumbnail | RenderLevel::Preview) => {
            super::jpeg::thumbnail(path, cache_dir, level, priority, cancellation)
        }
        (AssetKind::Png | AssetKind::Webp, RenderLevel::Thumbnail | RenderLevel::Preview) => {
            super::raster::thumbnail(path, cache_dir, level, priority, cancellation)
        }
        (AssetKind::Jpeg | AssetKind::Png | AssetKind::Webp, _) => register_original_resource(
            path,
            preview_result(path.to_owned(), PreviewKind::Original, level)?,
        ),
        (AssetKind::Raw, _) => raw::preview(
            path,
            cache_dir,
            level,
            priority,
            allow_interim,
            cancellation,
        ),
        (AssetKind::Heif, _) => heif::preview(
            path,
            cache_dir,
            level,
            priority,
            allow_interim,
            cancellation,
        ),
        (AssetKind::Tiff, _) => {
            system::preview(path, cache_dir, level, allow_interim, cancellation)
        }
    }?;
    if level == RenderLevel::Thumbnail && matches!(kind, AssetKind::Heif | AssetKind::Tiff) {
        super::thumbnail::ensure_thumbnail_delivery(path, cache_dir, result, priority, cancellation)
    } else {
        Ok(result)
    }
}
