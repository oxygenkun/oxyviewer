//! Internal presentation and cache-substitution contracts.
//!
//! These states are deliberately not an IPC schema. They prevent source size
//! alone from being treated as proof that two media representations are
//! interchangeable.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColorState {
    EmbeddedProfileOrUnknown,
    SrgbWithIcc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArtifactContract {
    CameraPreview,
    RawDevelopedSrgb,
    HeifPrimarySrgb,
    HeifPrimaryUnconverted,
}

impl ArtifactContract {
    pub(crate) const fn color(self) -> ColorState {
        match self {
            Self::CameraPreview | Self::HeifPrimaryUnconverted => {
                ColorState::EmbeddedProfileOrUnknown
            }
            Self::RawDevelopedSrgb | Self::HeifPrimarySrgb => ColorState::SrgbWithIcc,
        }
    }
}

pub(crate) const CAMERA_JPEG: ArtifactContract = ArtifactContract::CameraPreview;
pub(crate) const RAW_DEVELOPED_JPEG: ArtifactContract = ArtifactContract::RawDevelopedSrgb;
pub(crate) const HEIF_DECODED_JPEG: ArtifactContract = ArtifactContract::HeifPrimarySrgb;
pub(crate) const HEIF_UNCONVERTED_JPEG: ArtifactContract = ArtifactContract::HeifPrimaryUnconverted;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developed_artifacts_require_srgb_icc() {
        assert_eq!(HEIF_DECODED_JPEG.color(), ColorState::SrgbWithIcc);
        assert_eq!(RAW_DEVELOPED_JPEG.color(), ColorState::SrgbWithIcc);
        assert_eq!(CAMERA_JPEG.color(), ColorState::EmbeddedProfileOrUnknown);
        assert_eq!(
            HEIF_UNCONVERTED_JPEG.color(),
            ColorState::EmbeddedProfileOrUnknown
        );
    }
}
