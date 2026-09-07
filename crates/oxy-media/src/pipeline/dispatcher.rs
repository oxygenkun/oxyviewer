use super::{
    heif_preview,
    planner::{
        BackendCapabilities, DecodePlan, DecodeStep, PlannedPriority, Platform, Request, plan,
    },
    raw, system,
};
use crate::{DecodePriority, MediaError, media_source::preview_result, probe::ProbedSource};
use oxy_domain::{PreviewKind, PreviewResult, RenderLevel};
use std::path::{Path, PathBuf};

fn original(path: PathBuf) -> Result<PreviewResult, MediaError> {
    preview_result(path, PreviewKind::Original)
}

/// Convert the IPC-level [`PreviewPriority`] into the unified
/// [`DecodePriority`]. Kept here (in oxy-media) so the Tauri layer does not
/// need to know about the gate vocabulary.
pub fn decode_priority_for(priority: oxy_domain::PreviewPriority) -> DecodePriority {
    use oxy_domain::PreviewPriority;
    match priority {
        PreviewPriority::Preload => DecodePriority::Background,
        PreviewPriority::Nearby => DecodePriority::Background,
        PreviewPriority::Visible => DecodePriority::Visible,
        PreviewPriority::Loupe => DecodePriority::Foreground,
    }
}

const fn current_platform() -> Platform {
    #[cfg(target_os = "windows")]
    {
        Platform::Windows
    }
    #[cfg(target_os = "macos")]
    {
        Platform::Macos
    }
    #[cfg(target_os = "linux")]
    {
        Platform::Linux
    }
}

/// Unified preview dispatcher. Callers request a semantic [`RenderLevel`];
/// this module alone chooses the platform/format-specific decoder and size.
pub fn preview(
    path: &Path,
    cache_dir: &Path,
    level: RenderLevel,
    priority: DecodePriority,
    kind: oxy_domain::AssetKind,
) -> Result<PreviewResult, MediaError> {
    // Preserve the pre-planner executor's optimistic route set. Runtime
    // adapter failures still flow through the same ordered fallbacks and retain
    // their existing diagnostics; simulated capabilities are tested in the
    // pure planner without making directory discovery probe media contents.
    let probed_source = if kind == oxy_domain::AssetKind::Heif
        && matches!(level, RenderLevel::Thumbnail | RenderLevel::Preview)
    {
        heif_preview::probe_heif_for_preview(path, cache_dir)
    } else {
        ProbedSource::unprobed(kind)
    };
    let decode_plan = plan(
        probed_source.facts,
        Request {
            level,
            platform: current_platform(),
        },
        BackendCapabilities::configured_routes(),
    );
    let mut result = execute_decode_plan(path, cache_dir, priority, decode_plan, &probed_source)?;
    result.render_level = Some(level);
    Ok(result)
}

fn execute_decode_plan(
    path: &Path,
    cache_dir: &Path,
    request_priority: DecodePriority,
    decode_plan: DecodePlan,
    probed_source: &ProbedSource,
) -> Result<PreviewResult, MediaError> {
    let DecodePlan::Attempts { first, on_failure } = decode_plan else {
        return Err(MediaError::NativeDecoderUnavailable);
    };
    match (first, on_failure) {
        (
            DecodeStep::RawPreview { max_size },
            Some(DecodeStep::SystemPreview {
                max_size: fallback_size,
            }),
        ) => match raw::preview_with_priority(path, cache_dir, max_size, request_priority) {
            result @ Ok(_) | result @ Err(MediaError::Cancelled) => result,
            Err(error) => {
                // Preserve the original LibRaw error when the system fallback
                // also fails so diagnostics remain actionable.
                match system::preview(path, cache_dir, fallback_size) {
                    Ok(result) => Ok(result),
                    Err(system_error) => Err(MediaError::LibRaw {
                        path: path.to_owned(),
                        message: format!("{error}; fallback failed: {system_error}"),
                    }),
                }
            }
        },
        (DecodeStep::HeifFull, Some(fallback)) => match heif_preview::heif_full(path, cache_dir) {
            result @ Ok(_) | result @ Err(MediaError::Cancelled) => result,
            Err(error) => {
                // Preserve the foreground 8192px fallback after full-detail
                // HEIF failure so the loupe is not left empty.
                eprintln!(
                    "full-detail HEIF decode failed for {}: {error}",
                    path.display()
                );
                execute_decode_step(path, cache_dir, request_priority, fallback, probed_source)
            }
        },
        (first, None) => {
            execute_decode_step(path, cache_dir, request_priority, first, probed_source)
        }
        // The Stage-B planner only emits the two ordered fallback pairs above.
        // Treat a future unsupported pairing explicitly rather than silently
        // changing its error or fallback behavior.
        (_, Some(_)) => Err(MediaError::NativeDecoderUnavailable),
    }
}

fn execute_decode_step(
    path: &Path,
    cache_dir: &Path,
    request_priority: DecodePriority,
    step: DecodeStep,
    probed_source: &ProbedSource,
) -> Result<PreviewResult, MediaError> {
    match step {
        DecodeStep::Original => original(path.to_owned()),
        DecodeStep::RawPreview { max_size } => {
            raw::preview_with_priority(path, cache_dir, max_size, request_priority)
        }
        DecodeStep::RawFull => raw::full(path, cache_dir),
        DecodeStep::HeifPreview {
            max_size,
            try_fast_jpeg,
            allow_decode,
            priority,
        } => {
            let priority = match priority {
                PlannedPriority::Request => request_priority,
                PlannedPriority::Foreground => DecodePriority::Foreground,
            };
            heif_preview::heif_preview_with_options(
                path,
                cache_dir,
                max_size,
                priority,
                try_fast_jpeg,
                allow_decode,
                probed_source.heif_fast_jpeg.as_ref(),
            )
        }
        DecodeStep::HeifFull => heif_preview::heif_full(path, cache_dir),
        DecodeStep::SystemPreview { max_size } => system::preview(path, cache_dir, max_size),
    }
}
