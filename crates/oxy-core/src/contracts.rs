//! Public application contract exports. Definitions live with their feature.

pub use crate::app::types::*;
pub use crate::browse::types::*;
pub use crate::files::types::*;
pub use crate::jobs::types::*;
pub use crate::library::types::*;
pub use crate::metadata::types::*;
pub use crate::people::types::*;
pub use crate::resources::types::*;
pub use crate::tags::types::*;
pub use oxy_media::types::AnalysisRequirement;
pub use oxy_media::types::raw::*;
pub use oxy_media::types::{
    AccelerationKind, ArtifactFacts, AssetKind, ByteIntegrity, CaptureMetadata, DetailCoverage,
    DisplayDimensions, EditableMetadata, EncodedDimensions, FocusInfo, FocusRegion,
    HeifBackendKind, HeifCapabilities, HeifDecodeSession, HeifDecodeStatus, HeifDiagnostics,
    HeifTilePayload, ImageOperation, ImageOrigin, MediaPersistence, MediaResourceDescriptor,
    MediaSatisfaction, PickLabel, PixelDimensions, PreviewContentRect, PreviewDiagnostics,
    PreviewDisplaySize, PreviewGeometry, PreviewKind, PreviewPriority, PreviewResult, RenderLevel,
    ResourceRegistryStats, Sampling, SessionId, SourceImage,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_enrichment_is_required_only_for_metadata_filters() {
        assert!(!AssetQuery::default().needs_metadata_enrichment());

        // Cheap filters are answerable from the directory scan alone.
        assert!(
            !AssetQuery {
                search: Some("beach".into()),
                kind: Some(AssetKind::Raw),
                ..AssetQuery::default()
            }
            .needs_metadata_enrichment()
        );

        for filter in [
            AssetQuery {
                minimum_rating: Some(1),
                ..AssetQuery::default()
            },
            AssetQuery {
                color_labels: vec!["red".into()],
                ..AssetQuery::default()
            },
            AssetQuery {
                pick_labels: vec!["accepted".into()],
                ..AssetQuery::default()
            },
        ] {
            assert!(filter.needs_metadata_enrichment());
        }
    }

    #[test]
    fn metadata_patch_distinguishes_missing_fields_from_explicit_nulls() {
        let missing: MetadataPatch = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(missing.rating, None);
        assert_eq!(missing.color_label, None);
        assert_eq!(missing.pick_label, None);

        let cleared: MetadataPatch = serde_json::from_value(serde_json::json!({
            "rating": null,
            "colorLabel": null,
            "pickLabel": null
        }))
        .unwrap();
        assert_eq!(cleared.rating, Some(None));
        assert_eq!(cleared.color_label, Some(None));
        assert_eq!(cleared.pick_label, Some(None));

        let assigned: MetadataPatch = serde_json::from_value(serde_json::json!({
            "rating": 4,
            "colorLabel": "Purple",
            "pickLabel": "accepted"
        }))
        .unwrap();
        assert_eq!(assigned.rating, Some(Some(4)));
        assert_eq!(assigned.color_label, Some(Some("Purple".into())));
        assert_eq!(assigned.pick_label, Some(Some(PickLabel::Accepted)));
    }
}
