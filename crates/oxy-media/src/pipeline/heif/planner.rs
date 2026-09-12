//! HEIF candidate ordering and request policy; no file probes or decoder calls.
use crate::{
    cache::{
        ArtifactRequirement, DetailRequirement, ImageOrigin, OrientationRequirement,
        OrientationState, PresentationRequirement, SharpeningState,
    },
    pipeline::artifact::ArtifactCache,
};
use oxy_domain::{AccelerationKind, HeifBackendKind};

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

    pub(super) const fn label(self) -> &'static str {
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
    pub(super) const fn label(self) -> &'static str {
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
pub(super) struct PlannedDiagnostic {
    pub(super) before_candidate: usize,
    pub(super) diagnostic: AttemptDiagnostic,
}

#[derive(Clone, Debug)]
pub(crate) struct HeifBackendPlan {
    pub(crate) candidates: Vec<HeifBackend>,
    pub(super) diagnostics: Vec<PlannedDiagnostic>,
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
pub(super) struct BackendProbe {
    pub(super) backend: HeifBackend,
    pub(super) state: ProbeState,
}

#[derive(Clone, Debug)]
pub(super) enum ProbeState {
    Supported,
    Unsupported(String),
    Unavailable(String),
}

impl BackendProbe {
    pub(super) fn supported(backend: HeifBackend) -> Self {
        Self {
            backend,
            state: ProbeState::Supported,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // All profiles are covered by host-independent selector tests.
pub(super) enum Platform {
    Windows,
    Macos,
    Linux,
}

#[derive(Clone, Debug)]
pub(super) struct BackendProbes {
    pub(super) platform: BackendProbe,
    pub(super) ffmpeg: BackendProbe,
}

pub(super) fn plan_backends(
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

/// The full artifact is attempted before this compatibility preview plan.
pub(super) struct HeifPreviewPlan {
    pub max_size: u32,
    pub try_fast_jpeg: bool,
    pub require_native_detail: bool,
}

impl HeifPreviewPlan {
    pub fn for_level(level: oxy_domain::RenderLevel) -> Self {
        use oxy_domain::RenderLevel;
        Self {
            max_size: match level {
                RenderLevel::Thumbnail => 512,
                RenderLevel::Preview => 4_096,
                RenderLevel::Full => 8_192,
            },
            try_fast_jpeg: level != RenderLevel::Full,
            require_native_detail: level == RenderLevel::Full,
        }
    }
}

fn display_requirement() -> PresentationRequirement {
    PresentationRequirement {
        orientation: OrientationRequirement::DisplayCorrect,
        color: crate::cache::ColorRequirement::Any,
        sharpening: SharpeningState::None,
    }
}

pub(crate) fn preview_request(
    artifacts: &ArtifactCache,
    detail: DetailRequirement,
    allow_interim: bool,
) -> crate::cache::CacheRequest {
    artifacts.request(
        detail,
        ArtifactRequirement::AnyDisplay,
        display_requirement(),
        allow_interim,
    )
}

// Invalidate only the old Sony JPEG origin. Decoded previews must
// retain the full decoder's policy identity so concurrent requests share work.
pub(super) fn sony_embedded_request(
    artifacts: &ArtifactCache,
    detail: DetailRequirement,
) -> crate::cache::CacheRequest {
    let mut request = artifacts.request(
        detail,
        ArtifactRequirement::Exact(ImageOrigin::EmbeddedPreview),
        display_requirement(),
        true,
    );
    request.policy_revision = crate::cache::MEDIA_CACHE_POLICY_REVISION + 1;
    request
}

pub(crate) fn full_decoded_request(artifacts: &ArtifactCache) -> crate::cache::CacheRequest {
    full_display_request(artifacts, false)
}

pub(crate) const fn display_sharpening_state(enabled: bool) -> SharpeningState {
    if enabled {
        SharpeningState::Display
    } else {
        SharpeningState::None
    }
}

pub(super) fn full_display_request(
    artifacts: &ArtifactCache,
    display_sharpening: bool,
) -> crate::cache::CacheRequest {
    artifacts.request(
        DetailRequirement::NativeDetail,
        ArtifactRequirement::Exact(ImageOrigin::PrimaryImage),
        PresentationRequirement {
            orientation: OrientationRequirement::Exact(OrientationState::Applied),
            color: crate::cache::ColorRequirement::Any,
            sharpening: display_sharpening_state(display_sharpening),
        },
        false,
    )
}

pub(super) fn full_output_request(
    artifacts: &ArtifactCache,
    presentation: crate::cache::ArtifactPresentation,
) -> crate::cache::CacheRequest {
    artifacts.request(
        DetailRequirement::NativeDetail,
        ArtifactRequirement::Exact(ImageOrigin::PrimaryImage),
        PresentationRequirement {
            orientation: OrientationRequirement::Exact(presentation.orientation),
            color: match presentation.color {
                crate::cache::CacheColorState::Srgb => crate::cache::ColorRequirement::Srgb,
                crate::cache::CacheColorState::EmbeddedOrUnknown => {
                    crate::cache::ColorRequirement::Any
                }
            },
            sharpening: presentation.sharpening,
        },
        false,
    )
}
