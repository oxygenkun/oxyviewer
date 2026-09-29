//! Consumer-independent qualification for decoded frames and cache candidates.
use crate::{
    ArtifactPresentation, ColorRequirement, DetailRequirement, OrientationRequirement,
    PresentationRequirement, Satisfaction, SharpeningState,
};
use oxy_domain::{ArtifactFacts, PixelDimensions};

#[derive(Clone, Copy, Debug)]
pub struct MediaRequest {
    pub max_size: u32,
    pub detail: DetailRequirement,
    pub allow_interim: bool,
    pub presentation: PresentationRequirement,
}

impl MediaRequest {
    pub const fn unsharpened() -> PresentationRequirement {
        PresentationRequirement {
            orientation: OrientationRequirement::DisplayCorrect,
            color: ColorRequirement::Any,
            sharpening: SharpeningState::None,
        }
    }

    pub fn satisfaction(self, facts: &ArtifactFacts) -> Option<Satisfaction> {
        if !facts.is_consistent() || !facts.detail.covers_reference() {
            return None;
        }
        if self
            .detail
            .accepts(facts.detail.sampled_dimensions, facts.native_detail())
        {
            Some(Satisfaction::Satisfied)
        } else if self.allow_interim {
            Some(Satisfaction::Interim)
        } else {
            None
        }
    }

    pub fn accepts(self, facts: &ArtifactFacts) -> bool {
        self.satisfaction(facts).is_some()
    }

    pub fn full_frame(source: PixelDimensions, target: u32) -> Self {
        let long = source.width.max(source.height).max(1);
        let short = source.width.min(source.height);
        let target = target.min(long);
        // Camera JPEG and sensor active areas may differ slightly at the edges.
        // Keep the requested long edge strict; allow 1% on the derived short edge.
        Self {
            max_size: target,
            detail: DetailRequirement::MinimumDimensions {
                min_long_edge: target,
                min_short_edge: ((u64::from(short) * u64::from(target)) / u64::from(long) * 99
                    / 100)
                    .max(1) as u32,
            },
            allow_interim: false,
            presentation: Self::unsharpened(),
        }
    }
}

impl PresentationRequirement {
    pub(crate) fn accepts(self, actual: ArtifactPresentation) -> bool {
        !matches!(self.orientation, OrientationRequirement::Exact(expected) if actual.orientation != expected)
            && self.sharpening == actual.sharpening
            && (self.color == ColorRequirement::Any || actual.color == crate::CacheColorState::Srgb)
    }
}
