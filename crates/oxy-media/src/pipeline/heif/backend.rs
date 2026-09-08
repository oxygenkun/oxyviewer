//! HEIF backend probing, ordering, fallback execution, and diagnostics.

use crate::MediaError;
use oxy_domain::{AccelerationKind, HeifBackendKind, HeifCapabilities};
use std::path::Path;

// -----------------------------------------------------------------------------
// Plan model
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeifOperation {
    Preview,
    FullArtifact,
    Session,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeifBackend {
    CachedArtifact,
    Platform(HeifBackendKind),
    Ffmpeg,
    FfmpegRgbaFallback,
    Libheif,
}

impl HeifBackend {
    pub(crate) const fn kind(self) -> HeifBackendKind {
        match self {
            Self::CachedArtifact => HeifBackendKind::CachedArtifact,
            Self::Platform(kind) => kind,
            Self::Ffmpeg | Self::FfmpegRgbaFallback => HeifBackendKind::FfmpegSoftware,
            Self::Libheif => HeifBackendKind::LibheifSoftware,
        }
    }

    pub(crate) const fn acceleration(self) -> AccelerationKind {
        match self {
            Self::CachedArtifact => AccelerationKind::Software,
            Self::Platform(_) => AccelerationKind::Unknown,
            Self::Ffmpeg | Self::FfmpegRgbaFallback | Self::Libheif => AccelerationKind::Software,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::CachedArtifact => "cached full JPEG",
            Self::Platform(HeifBackendKind::WindowsWic) => "Windows WIC",
            Self::Platform(HeifBackendKind::AppleImageIo) => "Apple ImageIO",
            Self::Platform(_) => "platform HEIF backend",
            Self::Ffmpeg => "FFmpeg",
            Self::FfmpegRgbaFallback => "FFmpeg RGBA fallback",
            Self::Libheif => "libheif",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttemptOutcome {
    Unsupported,
    Unavailable,
    Corrupt,
    Io,
    Cancelled,
    DecodeFailed,
}

impl AttemptOutcome {
    const fn label(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Unavailable => "unavailable",
            Self::Corrupt => "corrupt",
            Self::Io => "io",
            Self::Cancelled => "cancelled",
            Self::DecodeFailed => "decode-failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttemptDiagnostic {
    pub(crate) backend: HeifBackend,
    pub(crate) outcome: AttemptOutcome,
    pub(crate) message: String,
}

#[derive(Clone, Debug)]
struct PlannedDiagnostic {
    before_candidate: usize,
    diagnostic: AttemptDiagnostic,
}

#[derive(Clone, Debug)]
pub(crate) struct HeifBackendPlan {
    pub(crate) candidates: Vec<HeifBackend>,
    diagnostics: Vec<PlannedDiagnostic>,
}

impl HeifBackendPlan {
    pub(crate) fn first(&self) -> HeifBackend {
        self.candidates
            .first()
            .copied()
            .unwrap_or(HeifBackend::Libheif)
    }

    pub(crate) fn cached_artifact() -> Self {
        Self {
            candidates: vec![HeifBackend::CachedArtifact],
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct BackendProbe {
    backend: HeifBackend,
    state: ProbeState,
}

#[derive(Clone, Debug)]
enum ProbeState {
    Supported,
    Unsupported(String),
    Unavailable(String),
}

impl BackendProbe {
    fn supported(backend: HeifBackend) -> Self {
        Self {
            backend,
            state: ProbeState::Supported,
        }
    }
}

pub(crate) struct BackendExecution<T> {
    pub(crate) value: T,
    pub(crate) backend: HeifBackend,
    pub(crate) diagnostics: Vec<AttemptDiagnostic>,
}

#[derive(Debug)]
pub(crate) struct BackendExecutionError {
    error: MediaError,
    diagnostics: Vec<AttemptDiagnostic>,
}

impl BackendExecutionError {
    pub(crate) fn into_media_error(self) -> MediaError {
        if matches!(self.error, MediaError::Cancelled) {
            return self.error;
        }
        MediaError::BackendAttempts {
            attempts: format_attempt_diagnostics(&self.diagnostics)
                .unwrap_or_else(|| "no compatible HEIF backend".into()),
            source: Box::new(self.error),
        }
    }
}

// -----------------------------------------------------------------------------
// Plan execution and diagnostics
// -----------------------------------------------------------------------------

pub(crate) fn execute_backend_plan<T>(
    plan: &HeifBackendPlan,
    mut cancelled: impl FnMut() -> bool,
    mut execute: impl FnMut(HeifBackend) -> Result<T, MediaError>,
) -> Result<BackendExecution<T>, BackendExecutionError> {
    let mut diagnostics = Vec::new();
    let mut planned_diagnostics = plan.diagnostics.iter().peekable();
    let mut last_error = None;
    for (candidate_index, backend) in plan.candidates.iter().enumerate() {
        while planned_diagnostics
            .peek()
            .is_some_and(|planned| planned.before_candidate <= candidate_index)
        {
            diagnostics.push(planned_diagnostics.next().unwrap().diagnostic.clone());
        }
        if cancelled() {
            return Err(BackendExecutionError {
                error: MediaError::Cancelled,
                diagnostics,
            });
        }
        match execute(*backend) {
            Ok(value) => {
                // Native calls are not necessarily preemptible. Reject their
                // late result rather than publishing it or starting cache work.
                if cancelled() {
                    return Err(BackendExecutionError {
                        error: MediaError::Cancelled,
                        diagnostics,
                    });
                }
                return Ok(BackendExecution {
                    value,
                    backend: *backend,
                    diagnostics,
                });
            }
            Err(MediaError::Cancelled) => {
                return Err(BackendExecutionError {
                    error: MediaError::Cancelled,
                    diagnostics,
                });
            }
            Err(error) => {
                diagnostics.push(AttemptDiagnostic {
                    backend: *backend,
                    outcome: classify_error(&error),
                    message: error.to_string(),
                });
                last_error = Some(error);
                // Cancellation terminates the plan. It must not be interpreted
                // as permission to try a slower compatibility backend.
                if cancelled() {
                    return Err(BackendExecutionError {
                        error: MediaError::Cancelled,
                        diagnostics,
                    });
                }
            }
        }
    }
    diagnostics.extend(planned_diagnostics.map(|planned| planned.diagnostic.clone()));
    Err(BackendExecutionError {
        error: last_error.unwrap_or(MediaError::NativeDecoderUnavailable),
        diagnostics,
    })
}

pub(crate) fn format_attempt_diagnostics(diagnostics: &[AttemptDiagnostic]) -> Option<String> {
    (!diagnostics.is_empty()).then(|| {
        diagnostics
            .iter()
            .map(|attempt| {
                format!(
                    "{} {}: {}",
                    attempt.backend.label(),
                    attempt.outcome.label(),
                    attempt.message
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    })
}

// -----------------------------------------------------------------------------
// Backend planning
// -----------------------------------------------------------------------------

pub(crate) fn backend_plan(path: &Path, operation: HeifOperation) -> HeifBackendPlan {
    select_backends(
        current_platform(),
        operation,
        production_probes(path, operation),
    )
}

pub(crate) fn capabilities() -> Vec<HeifCapabilities> {
    vec![
        platform_capability(),
        ffmpeg_capability(),
        libheif_capability(),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // All profiles are covered by host-independent selector tests.
enum Platform {
    Windows,
    Macos,
    Linux,
}

#[derive(Clone, Debug)]
struct BackendProbes {
    platform: BackendProbe,
    ffmpeg: BackendProbe,
}

fn select_backends(
    platform: Platform,
    operation: HeifOperation,
    probes: BackendProbes,
) -> HeifBackendPlan {
    let mut plan = HeifBackendPlan {
        candidates: Vec::with_capacity(3),
        diagnostics: Vec::new(),
    };
    match (platform, operation) {
        (Platform::Macos, HeifOperation::Preview) => push_probe(&mut plan, probes.platform),
        (Platform::Windows, HeifOperation::Preview) => push_probe(&mut plan, probes.ffmpeg),
        (Platform::Linux, HeifOperation::Preview) => {}
        (Platform::Macos, HeifOperation::FullArtifact) => {
            push_probe(&mut plan, probes.platform);
            push_probe(&mut plan, probes.ffmpeg);
            return plan;
        }
        (Platform::Windows | Platform::Linux, HeifOperation::FullArtifact) => {
            push_probe(&mut plan, probes.ffmpeg);
            return plan;
        }
        (Platform::Windows, HeifOperation::Session) => {
            let ffmpeg_supported = matches!(probes.ffmpeg.state, ProbeState::Supported);
            push_probe(&mut plan, probes.ffmpeg);
            if ffmpeg_supported {
                plan.candidates.push(HeifBackend::FfmpegRgbaFallback);
            }
            if !ffmpeg_supported {
                push_probe(&mut plan, probes.platform);
            }
        }
        (Platform::Macos, HeifOperation::Session) => {
            push_probe(&mut plan, probes.platform);
            push_probe(&mut plan, probes.ffmpeg);
        }
        (Platform::Linux, HeifOperation::Session) => push_probe(&mut plan, probes.ffmpeg),
    }
    plan.candidates.push(HeifBackend::Libheif);
    plan
}

fn push_probe(plan: &mut HeifBackendPlan, probe: BackendProbe) {
    match probe.state {
        ProbeState::Supported => plan.candidates.push(probe.backend),
        ProbeState::Unsupported(message) => plan.diagnostics.push(PlannedDiagnostic {
            before_candidate: plan.candidates.len(),
            diagnostic: AttemptDiagnostic {
                backend: probe.backend,
                outcome: AttemptOutcome::Unsupported,
                message,
            },
        }),
        ProbeState::Unavailable(message) => plan.diagnostics.push(PlannedDiagnostic {
            before_candidate: plan.candidates.len(),
            diagnostic: AttemptDiagnostic {
                backend: probe.backend,
                outcome: AttemptOutcome::Unavailable,
                message,
            },
        }),
    }
}

// -----------------------------------------------------------------------------
// Runtime probes and capability reporting
// -----------------------------------------------------------------------------

fn production_probes(path: &Path, operation: HeifOperation) -> BackendProbes {
    let unused_platform =
        || BackendProbe::supported(HeifBackend::Platform(platform_backend_kind()));
    let unused_ffmpeg = || BackendProbe::supported(HeifBackend::Ffmpeg);
    match (current_platform(), operation) {
        (Platform::Macos, HeifOperation::Preview) => BackendProbes {
            platform: actual_platform_probe(path),
            ffmpeg: unused_ffmpeg(),
        },
        (Platform::Macos, HeifOperation::FullArtifact | HeifOperation::Session) => {
            let platform = actual_platform_probe(path);
            let ffmpeg = if matches!(platform.state, ProbeState::Supported) {
                // Preserve the old fast path: do not inspect the fallback
                // backend unless the preferred native backend fails later.
                unused_ffmpeg()
            } else {
                actual_ffmpeg_probe(path)
            };
            BackendProbes { platform, ffmpeg }
        }
        (Platform::Linux, HeifOperation::Preview) => BackendProbes {
            platform: unused_platform(),
            ffmpeg: unused_ffmpeg(),
        },
        (Platform::Windows, HeifOperation::Preview)
        | (Platform::Windows | Platform::Linux, HeifOperation::FullArtifact)
        | (Platform::Linux, HeifOperation::Session) => BackendProbes {
            platform: unused_platform(),
            ffmpeg: actual_ffmpeg_probe(path),
        },
        (Platform::Windows, HeifOperation::Session) => {
            let ffmpeg = actual_ffmpeg_probe(path);
            let platform = if !matches!(ffmpeg.state, ProbeState::Supported) {
                actual_platform_probe(path)
            } else {
                unused_platform()
            };
            BackendProbes { platform, ffmpeg }
        }
    }
}

fn actual_ffmpeg_probe(path: &Path) -> BackendProbe {
    let capability = ffmpeg_capability();
    if capability.available {
        support_probe(
            HeifBackend::Ffmpeg,
            crate::backends::ffmpeg_heif::can_decode(path),
        )
    } else {
        BackendProbe {
            backend: HeifBackend::Ffmpeg,
            state: ProbeState::Unavailable(
                capability
                    .detail
                    .unwrap_or_else(|| "FFmpeg is unavailable".into()),
            ),
        }
    }
}

fn actual_platform_probe(path: &Path) -> BackendProbe {
    let backend = HeifBackend::Platform(platform_backend_kind());
    let capability = platform_capability();
    if capability.available {
        platform_support_probe(backend, path)
    } else {
        BackendProbe {
            backend,
            state: ProbeState::Unavailable(
                capability
                    .detail
                    .unwrap_or_else(|| "platform decoder is unavailable".into()),
            ),
        }
    }
}

fn support_probe(backend: HeifBackend, support: Result<(), MediaError>) -> BackendProbe {
    match support {
        Ok(()) => BackendProbe::supported(backend),
        Err(error) => BackendProbe {
            backend,
            state: ProbeState::Unsupported(error.to_string()),
        },
    }
}

fn platform_support_probe(backend: HeifBackend, path: &Path) -> BackendProbe {
    #[cfg(target_os = "windows")]
    return support_probe(backend, crate::backends::windows_wic::can_decode(path));
    #[cfg(target_os = "macos")]
    return support_probe(backend, crate::backends::apple_image_io::can_decode(path));
    #[cfg(target_os = "linux")]
    {
        let _ = path;
        BackendProbe {
            backend,
            state: ProbeState::Unavailable("native HEIF adapter is not built".into()),
        }
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

pub(crate) fn ffmpeg_capability() -> HeifCapabilities {
    match crate::backends::ffmpeg_heif::capability() {
        Ok(()) => HeifCapabilities {
            backend: HeifBackendKind::FfmpegSoftware,
            acceleration: AccelerationKind::Software,
            available: true,
            detail: Some("FFmpeg HEIF tile-grid decoder".into()),
        },
        Err(error) => HeifCapabilities {
            backend: HeifBackendKind::FfmpegSoftware,
            acceleration: AccelerationKind::Software,
            available: false,
            detail: Some(error.to_string()),
        },
    }
}

pub(crate) fn libheif_capability() -> HeifCapabilities {
    HeifCapabilities {
        backend: HeifBackendKind::LibheifSoftware,
        acceleration: AccelerationKind::Software,
        available: true,
        detail: Some("portable compatibility backend".into()),
    }
}

const fn platform_backend_kind() -> HeifBackendKind {
    #[cfg(target_os = "windows")]
    {
        HeifBackendKind::WindowsWic
    }
    #[cfg(target_os = "macos")]
    {
        HeifBackendKind::AppleImageIo
    }
    #[cfg(target_os = "linux")]
    {
        HeifBackendKind::LinuxVaapi
    }
}

pub(crate) fn platform_capability() -> HeifCapabilities {
    #[cfg(target_os = "windows")]
    return match crate::backends::windows_wic::capability() {
        Ok(()) => HeifCapabilities {
            backend: HeifBackendKind::WindowsWic,
            acceleration: AccelerationKind::Unknown,
            available: true,
            detail: Some(
                "Windows WIC HEIF decoder installed; GPU acceleration is not verified".into(),
            ),
        },
        Err(error) => HeifCapabilities {
            backend: HeifBackendKind::WindowsWic,
            acceleration: AccelerationKind::Unknown,
            available: false,
            detail: Some(error.to_string()),
        },
    };
    #[cfg(target_os = "macos")]
    return match crate::backends::apple_image_io::capability() {
        Ok(()) => HeifCapabilities {
            backend: HeifBackendKind::AppleImageIo,
            acceleration: AccelerationKind::Unknown,
            available: true,
            detail: Some(
                "Apple ImageIO native HEIF decoder; hardware use is selected internally and cannot be verified".into(),
            ),
        },
        Err(error) => HeifCapabilities {
            backend: HeifBackendKind::AppleImageIo,
            acceleration: AccelerationKind::Unknown,
            available: false,
            detail: Some(error.to_string()),
        },
    };
    #[cfg(target_os = "linux")]
    {
        HeifCapabilities {
            backend: HeifBackendKind::LinuxVaapi,
            acceleration: AccelerationKind::Hardware,
            available: false,
            detail: Some("native hardware adapter is not available in this build".into()),
        }
    }
}

// -----------------------------------------------------------------------------
// Error classification
// -----------------------------------------------------------------------------

fn classify_error(error: &MediaError) -> AttemptOutcome {
    match error {
        MediaError::NativeDecoderUnavailable => AttemptOutcome::Unavailable,
        MediaError::Io(_) => AttemptOutcome::Io,
        MediaError::Image(_) => AttemptOutcome::Corrupt,
        MediaError::BackendAttempts { source, .. } => classify_error(source),
        MediaError::Cancelled
        | MediaError::StaleCacheGeneration
        | MediaError::StaleSourceRevision => AttemptOutcome::Cancelled,
        MediaError::NativeDecode { .. }
        | MediaError::LibRaw { .. }
        | MediaError::Heif { .. }
        | MediaError::Color(_)
        | MediaError::CacheManifest(_)
        | MediaError::CacheArtifact(_)
        | MediaError::ResourceBudgetExhausted { .. }
        | MediaError::PreviewGenerationFailed { .. } => AttemptOutcome::DecodeFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        io,
        path::PathBuf,
        sync::atomic::{AtomicBool, Ordering},
    };

    fn probe(backend: HeifBackend, state: ProbeState) -> BackendProbe {
        BackendProbe { backend, state }
    }

    fn probes(platform_state: ProbeState, ffmpeg_state: ProbeState) -> BackendProbes {
        BackendProbes {
            platform: probe(
                HeifBackend::Platform(HeifBackendKind::AppleImageIo),
                platform_state,
            ),
            ffmpeg: probe(HeifBackend::Ffmpeg, ffmpeg_state),
        }
    }

    #[test]
    fn selector_preserves_preview_and_session_backend_order() {
        let supported = || ProbeState::Supported;
        let mac_preview = select_backends(
            Platform::Macos,
            HeifOperation::Preview,
            probes(supported(), supported()),
        );
        assert_eq!(
            mac_preview.candidates,
            [
                HeifBackend::Platform(HeifBackendKind::AppleImageIo),
                HeifBackend::Libheif,
            ]
        );
        let mac_session = select_backends(
            Platform::Macos,
            HeifOperation::Session,
            probes(supported(), supported()),
        );
        assert_eq!(
            mac_session.candidates,
            [
                HeifBackend::Platform(HeifBackendKind::AppleImageIo),
                HeifBackend::Ffmpeg,
                HeifBackend::Libheif,
            ]
        );
        let windows_session = select_backends(
            Platform::Windows,
            HeifOperation::Session,
            probes(supported(), supported()),
        );
        assert_eq!(
            windows_session.candidates,
            [
                HeifBackend::Ffmpeg,
                HeifBackend::FfmpegRgbaFallback,
                HeifBackend::Libheif,
            ]
        );
        let linux_session = select_backends(
            Platform::Linux,
            HeifOperation::Session,
            probes(supported(), supported()),
        );
        assert_eq!(
            linux_session.candidates,
            [HeifBackend::Ffmpeg, HeifBackend::Libheif]
        );
        let mac_artifact = select_backends(
            Platform::Macos,
            HeifOperation::FullArtifact,
            probes(supported(), supported()),
        );
        assert_eq!(
            mac_artifact.candidates,
            [
                HeifBackend::Platform(HeifBackendKind::AppleImageIo),
                HeifBackend::Ffmpeg,
            ]
        );
    }

    #[test]
    fn unavailable_and_unsupported_backends_are_diagnosed() {
        let plan = select_backends(
            Platform::Macos,
            HeifOperation::Session,
            probes(
                ProbeState::Unavailable("framework missing".into()),
                ProbeState::Unsupported("codec profile".into()),
            ),
        );
        assert_eq!(plan.candidates, [HeifBackend::Libheif]);
        assert_eq!(plan.diagnostics.len(), 2);
        assert_eq!(
            plan.diagnostics[0].diagnostic.outcome,
            AttemptOutcome::Unavailable
        );
        assert_eq!(
            plan.diagnostics[1].diagnostic.outcome,
            AttemptOutcome::Unsupported
        );
    }

    #[test]
    fn unused_later_backend_is_not_reported_as_a_fallback() {
        let plan = select_backends(
            Platform::Macos,
            HeifOperation::Session,
            probes(
                ProbeState::Supported,
                ProbeState::Unsupported("not a grid".into()),
            ),
        );
        let result = execute_backend_plan(
            &plan,
            || false,
            |backend| {
                assert!(matches!(backend, HeifBackend::Platform(_)));
                Ok(())
            },
        )
        .unwrap();
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn executor_records_failures_and_uses_next_backend() {
        let plan = HeifBackendPlan {
            candidates: vec![HeifBackend::Ffmpeg, HeifBackend::Libheif],
            diagnostics: Vec::new(),
        };
        let result = execute_backend_plan(
            &plan,
            || false,
            |backend| match backend {
                HeifBackend::Ffmpeg => Err(MediaError::Io(io::Error::other("read failed"))),
                HeifBackend::Libheif => Ok(42),
                HeifBackend::CachedArtifact
                | HeifBackend::Platform(_)
                | HeifBackend::FfmpegRgbaFallback => unreachable!(),
            },
        )
        .unwrap();
        assert_eq!(result.value, 42);
        assert_eq!(result.backend, HeifBackend::Libheif);
        assert_eq!(result.diagnostics[0].outcome, AttemptOutcome::Io);
    }

    #[test]
    fn executor_classifies_corrupt_input_and_preserves_all_attempts_on_failure() {
        let plan = HeifBackendPlan {
            candidates: vec![HeifBackend::Ffmpeg, HeifBackend::Libheif],
            diagnostics: Vec::new(),
        };
        let error = execute_backend_plan::<()>(
            &plan,
            || false,
            |backend| match backend {
                HeifBackend::Ffmpeg => Err(MediaError::Image(image::ImageError::Decoding(
                    image::error::DecodingError::new(
                        image::error::ImageFormatHint::Unknown,
                        "bad payload",
                    ),
                ))),
                HeifBackend::Libheif => Err(MediaError::Heif {
                    path: PathBuf::from("broken.hif"),
                    message: "invalid item".into(),
                }),
                HeifBackend::CachedArtifact
                | HeifBackend::Platform(_)
                | HeifBackend::FfmpegRgbaFallback => unreachable!(),
            },
        )
        .err()
        .unwrap();
        assert_eq!(error.diagnostics.len(), 2);
        assert_eq!(error.diagnostics[0].outcome, AttemptOutcome::Corrupt);
        assert_eq!(error.diagnostics[1].outcome, AttemptOutcome::DecodeFailed);
        assert!(matches!(
            error.into_media_error(),
            MediaError::BackendAttempts { .. }
        ));
    }

    #[test]
    fn cancellation_short_circuits_fallback_and_discards_late_success() {
        assert_eq!(
            classify_error(&MediaError::Cancelled),
            AttemptOutcome::Cancelled
        );
        let plan = HeifBackendPlan {
            candidates: vec![HeifBackend::Ffmpeg, HeifBackend::Libheif],
            diagnostics: Vec::new(),
        };
        let cancelled = AtomicBool::new(false);
        let calls = Cell::new(0);
        let result = execute_backend_plan(
            &plan,
            || cancelled.load(Ordering::Acquire),
            |backend| {
                calls.set(calls.get() + 1);
                cancelled.store(true, Ordering::Release);
                if backend == HeifBackend::Ffmpeg {
                    Err(MediaError::NativeDecode {
                        backend: "FFmpeg",
                        message: "failed".into(),
                    })
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(
            result.err().unwrap().into_media_error(),
            MediaError::Cancelled
        ));
        assert_eq!(calls.get(), 1);

        cancelled.store(false, Ordering::Release);
        let result = execute_backend_plan(
            &plan,
            || cancelled.load(Ordering::Acquire),
            |_| {
                cancelled.store(true, Ordering::Release);
                Ok(())
            },
        );
        assert!(matches!(
            result.err().unwrap().into_media_error(),
            MediaError::Cancelled
        ));
    }
}
