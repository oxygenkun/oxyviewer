use super::{heif, input::MediaPixels, raw, system};
use crate::{
    MediaError, MediaRequest, SourceRevision,
    decode_control::{self, DecodePriority},
    media_source::preview_result,
    pipeline::artifact::register_original_resource,
};
use oxy_domain::PixelDimensions;
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

pub(crate) fn prepare_pixels(
    path: &Path,
    kind: AssetKind,
    revision: &SourceRevision,
    dimensions: PixelDimensions,
    request: MediaRequest,
    cancellation: &CancellationToken,
) -> Result<MediaPixels, MediaError> {
    let target = request.max_size;
    match kind {
        AssetKind::Png | AssetKind::Webp => {
            super::raster::decode_pixels(path, revision, target, cancellation)
        }
        AssetKind::Tiff => super::system::decode_pixels(path, revision, target, cancellation),
        AssetKind::Jpeg => super::jpeg::decode_pixels(path, revision, request, cancellation),
        AssetKind::Raw => {
            super::raw::decode_pixels(path, revision, dimensions, request, cancellation)
        }
        AssetKind::Heif => {
            let cost = super::pixels::estimate(dimensions, 8, revision.size_bytes)?;
            let _permit =
                decode_control::acquire_pixel_input(cost, &|| cancellation.is_cancelled())?;
            // Same frame selector as viewing; only the requested detail differs.
            let decoded =
                crate::pipeline::heif::artifact::decode_frame(path, request, cancellation)?;
            super::pixels::finish(
                decoded.image,
                decoded.facts,
                revision,
                decoded.presentation.color,
                None,
                target,
                cancellation,
            )
        }
    }
}

/// Encoded primary files already have cheap direct/scaled paths; prefer their
/// source samples. Native camera containers benefit from qualified artifacts.
pub(crate) const fn prefer_cached_pixels(kind: AssetKind) -> bool {
    matches!(kind, AssetKind::Raw | AssetKind::Heif | AssetKind::Tiff)
}
