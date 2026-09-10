use oxy_domain::{AssetKind, RenderLevel};

pub(crate) const RAW_PREVIEW: &str = "raw-native-v8-camera-or-developed-srgb";
pub(crate) const RAW_FULL: &str = "raw-native-full-v4-srgb";
pub(crate) const HEIF_PREVIEW: &str = "heif-native-preview-v10-content-geometry";
pub(crate) const HEIF_FULL: &str = "heif-source-jpeg-v2";
pub(crate) const SYSTEM_PREVIEW: &str = "apple-image-io-preview-v3-jpeg";
const ORIGINAL: &str = "original-v1";

/// The same policy identity is used by cache keys and persisted projections.
/// Changing media behavior therefore cannot leave a ready projection pointing
/// at an artifact produced by an older policy.
pub const fn preview_policy_revision(kind: AssetKind, level: RenderLevel) -> &'static str {
    match (kind, level) {
        (AssetKind::Raw, RenderLevel::Full) => RAW_FULL,
        (AssetKind::Raw, _) => RAW_PREVIEW,
        (AssetKind::Heif, RenderLevel::Full) => HEIF_FULL,
        (AssetKind::Heif, _) => HEIF_PREVIEW,
        (AssetKind::Tiff, _) => SYSTEM_PREVIEW,
        (AssetKind::Jpeg | AssetKind::Png | AssetKind::Webp, _) => ORIGINAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_and_artifact_policies_share_one_source() {
        assert_eq!(
            preview_policy_revision(AssetKind::Raw, RenderLevel::Preview),
            RAW_PREVIEW
        );
        assert_eq!(
            preview_policy_revision(AssetKind::Raw, RenderLevel::Full),
            RAW_FULL
        );
        assert_eq!(
            preview_policy_revision(AssetKind::Heif, RenderLevel::Preview),
            HEIF_PREVIEW
        );
        assert_eq!(
            preview_policy_revision(AssetKind::Heif, RenderLevel::Full),
            HEIF_FULL
        );
        assert_eq!(
            preview_policy_revision(AssetKind::Tiff, RenderLevel::Full),
            SYSTEM_PREVIEW
        );
    }
}
