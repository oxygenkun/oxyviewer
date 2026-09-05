//! Internal presentation and cache-substitution contracts.
//!
//! These states are deliberately not an IPC schema. They prevent source size
//! alone from being treated as proof that two media representations are
//! interchangeable.

use crate::ImageDimensions;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepresentationSource {
    CameraPreview,
    RawDevelopment,
    HeifPrimary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OrientationState {
    EncodedMetadata,
    AppliedToPixels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColorState {
    EmbeddedProfileOrUnknown,
    SrgbWithIcc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DynamicRange {
    Sdr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactContract {
    pub(crate) representation: RepresentationSource,
    pub(crate) orientation: OrientationState,
    pub(crate) color: ColorState,
    pub(crate) dynamic_range: DynamicRange,
    pub(crate) policy_version: u8,
}

pub(crate) const CAMERA_JPEG: ArtifactContract = ArtifactContract {
    representation: RepresentationSource::CameraPreview,
    orientation: OrientationState::EncodedMetadata,
    color: ColorState::EmbeddedProfileOrUnknown,
    dynamic_range: DynamicRange::Sdr,
    policy_version: 1,
};

pub(crate) const RAW_DEVELOPED_JPEG: ArtifactContract = ArtifactContract {
    representation: RepresentationSource::RawDevelopment,
    orientation: OrientationState::AppliedToPixels,
    color: ColorState::SrgbWithIcc,
    dynamic_range: DynamicRange::Sdr,
    policy_version: 1,
};

pub(crate) const HEIF_DECODED_JPEG: ArtifactContract = ArtifactContract {
    representation: RepresentationSource::HeifPrimary,
    orientation: OrientationState::AppliedToPixels,
    color: ColorState::SrgbWithIcc,
    dynamic_range: DynamicRange::Sdr,
    policy_version: 1,
};

/// A camera JPEG may stand in for pixel inspection only under the explicit
/// camera-rendered policy and only when it covers the RAW display dimensions.
/// It is never reclassified as a RAW development result.
pub(crate) fn camera_preview_can_satisfy_raw_full(
    contract: ArtifactContract,
    candidate: ImageDimensions,
    source: ImageDimensions,
) -> bool {
    contract == CAMERA_JPEG && covers_display(candidate, source, 90)
}

fn covers_display(candidate: ImageDimensions, source: ImageDimensions, percent: u64) -> bool {
    let mut candidate_edges = [candidate.width, candidate.height];
    let mut source_edges = [source.width, source.height];
    candidate_edges.sort_unstable();
    source_edges.sort_unstable();
    candidate_edges
        .into_iter()
        .zip(source_edges)
        .all(|(candidate, source)| u64::from(candidate) * 100 >= u64::from(source) * percent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representation_contract_not_size_rank_controls_raw_full_substitution() {
        let candidate = ImageDimensions {
            width: 7_008,
            height: 4_672,
        };
        let source = ImageDimensions {
            width: 4_688,
            height: 7_028,
        };
        assert!(camera_preview_can_satisfy_raw_full(
            CAMERA_JPEG,
            candidate,
            source
        ));
        assert!(!camera_preview_can_satisfy_raw_full(
            RAW_DEVELOPED_JPEG,
            candidate,
            source
        ));
        assert!(!camera_preview_can_satisfy_raw_full(
            CAMERA_JPEG,
            ImageDimensions {
                width: 1_616,
                height: 1_080,
            },
            source
        ));
    }

    #[test]
    fn decoded_heif_contract_is_explicitly_oriented_sdr_srgb() {
        assert_eq!(
            HEIF_DECODED_JPEG.orientation,
            OrientationState::AppliedToPixels
        );
        assert_eq!(HEIF_DECODED_JPEG.color, ColorState::SrgbWithIcc);
        assert_eq!(HEIF_DECODED_JPEG.dynamic_range, DynamicRange::Sdr);
    }
}
