use oxy_domain::{AssetKind, RenderLevel};

pub(crate) const RAW_PREVIEW: &str = "raw-native-v11-largest-camera-jpeg";
pub(crate) const RAW_FULL: &str = "raw-native-full-v10-camera-jpeg-interim";
pub(crate) const RAW_THUMBNAIL: &str = "raw-thumbnail-v2-decoder-pixels";
pub(crate) const HEIF_PREVIEW: &str = "heif-native-preview-v10-content-geometry-facts-v1";
pub(crate) const HEIF_FULL: &str = "heif-source-jpeg-v2-facts-v1";
pub(crate) const HEIF_THUMBNAIL: &str = "heif-native-thumbnail-v11-bounded-delivery-facts-v1";
pub(crate) const SYSTEM_PREVIEW: &str = "apple-image-io-preview-v3-jpeg-facts-v1";
pub(crate) const COMPATIBLE_THUMBNAIL: &str = "compatible-thumbnail-v1-bounded-jpeg-facts-v1";
const ORIGINAL: &str = "original-v1-facts-v1";
pub(crate) const JPEG_THUMBNAIL: &str = "jpeg-thumbnail-v1-bounded-mpf-facts-v1";
pub(crate) const RASTER_THUMBNAIL: &str = "raster-thumbnail-v1-bounded-png-facts-v1";

/// The same policy identity is used by cache keys and persisted projections.
/// Changing media behavior therefore cannot leave a ready projection pointing
/// at an artifact produced by an older policy.
pub const fn preview_policy_revision(kind: AssetKind, level: RenderLevel) -> &'static str {
    match (kind, level) {
        (AssetKind::Raw, RenderLevel::Full) => RAW_FULL,
        (AssetKind::Raw, RenderLevel::Thumbnail) => RAW_THUMBNAIL,
        (AssetKind::Raw, _) => RAW_PREVIEW,
        (AssetKind::Heif, RenderLevel::Full) => HEIF_FULL,
        (AssetKind::Heif, RenderLevel::Thumbnail) => HEIF_THUMBNAIL,
        (AssetKind::Heif, _) => HEIF_PREVIEW,
        (AssetKind::Tiff, _) => SYSTEM_PREVIEW,
        (AssetKind::Jpeg, RenderLevel::Thumbnail | RenderLevel::Preview) => JPEG_THUMBNAIL,
        (AssetKind::Png | AssetKind::Webp, RenderLevel::Thumbnail | RenderLevel::Preview) => {
            RASTER_THUMBNAIL
        }
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
