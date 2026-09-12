use crate::MediaError;
use oxy_fs::observe_file;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub const MEDIA_CACHE_POLICY_REVISION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRevision {
    pub canonical_path: PathBuf,
    pub file_identity: String,
    pub size_bytes: u64,
    pub modified: String,
    pub revision_id: String,
}

impl SourceRevision {
    pub fn observe(path: &std::path::Path) -> Result<Self, MediaError> {
        let observed = observe_file(path)?;
        let mut hasher = Sha256::new();
        hasher.update(b"oxy-media-source-revision-v2\0");
        update_path(&mut hasher, &observed.canonical_path);
        hasher.update(observed.file_identity.as_bytes());
        hasher.update([0]);
        hasher.update(observed.size_bytes.to_le_bytes());
        hasher.update(observed.modified.as_bytes());
        let revision_id = format!("{:x}", hasher.finalize());
        Ok(Self {
            canonical_path: observed.canonical_path,
            file_identity: observed.file_identity,
            size_bytes: observed.size_bytes,
            modified: observed.modified,
            revision_id,
        })
    }
}

pub use oxy_domain::PixelDimensions;

pub use oxy_domain::ImageOrigin;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OrientationState {
    Metadata,
    Applied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorState {
    EmbeddedOrUnknown,
    Srgb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SharpeningState {
    None,
    Display,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPresentation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<oxy_domain::PreviewGeometry>,
    pub orientation: OrientationState,
    pub color: ColorState,
    pub sharpening: SharpeningState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorRequirement {
    Any,
    Srgb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrientationRequirement {
    DisplayCorrect,
    Exact(OrientationState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationRequirement {
    pub orientation: OrientationRequirement,
    pub color: ColorRequirement,
    pub sharpening: SharpeningState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailRequirement {
    Display {
        min_long_edge: u32,
    },
    /// Requires a true native-detail artifact without probing source dimensions.
    NativeDetail,
    /// Requires both display-space edges, independently of native-detail provenance.
    MinimumDimensions {
        min_long_edge: u32,
        min_short_edge: u32,
    },
}

impl DetailRequirement {
    pub(crate) fn accepts(
        self,
        dimensions: oxy_domain::DisplayDimensions,
        native_detail: bool,
    ) -> bool {
        let dimensions = dimensions.0;
        match self {
            DetailRequirement::Display { min_long_edge } => {
                native_detail || long_edge(dimensions) >= min_long_edge
            }
            DetailRequirement::NativeDetail => native_detail,
            DetailRequirement::MinimumDimensions {
                min_long_edge,
                min_short_edge,
            } => {
                dimensions.width.max(dimensions.height) >= min_long_edge
                    && dimensions.width.min(dimensions.height) >= min_short_edge
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactRequirement {
    /// A thumbnail policy is a delivery contract, not a minimum quality rank.
    BoundedThumbnail {
        target: &'static str,
    },
    AnyDisplay,
    Exact(ImageOrigin),
    ExactVariant {
        origin: ImageOrigin,
        target: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantIdentity {
    pub presentation: ArtifactPresentation,
    pub policy_revision: u32,
    pub target: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaArtifact {
    pub artifact_id: String,
    pub source_revision: SourceRevision,
    pub variant: VariantIdentity,
    pub facts: oxy_domain::ArtifactFacts,
    pub byte_size: u64,
    pub media_type: String,
    pub location: ArtifactLocation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum ArtifactLocation {
    Managed(PathBuf),
    Original(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheRequest {
    pub source_revision: SourceRevision,
    pub detail: DetailRequirement,
    pub artifact: ArtifactRequirement,
    pub presentation: PresentationRequirement,
    pub policy_revision: u32,
    pub allow_interim: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Satisfaction {
    Satisfied,
    Interim,
}

/// Matches real artifact capabilities. Semantic render levels are deliberately
/// absent: callers translate them into format-aware requirements once.
pub fn satisfies(artifact: &MediaArtifact, request: &CacheRequest) -> Option<Satisfaction> {
    // Current preview requests cover the complete reference image, never a crop/tile.
    if !artifact.facts.is_consistent()
        || !artifact.facts.detail.covers_reference()
        || artifact.facts.source.revision_id != artifact.source_revision.revision_id
        || artifact.source_revision != request.source_revision
        || artifact.variant.policy_revision != request.policy_revision
        || (matches!(
            request.presentation.orientation,
            OrientationRequirement::Exact(expected)
                if artifact.variant.presentation.orientation != expected
        ))
        || artifact.variant.presentation.sharpening != request.presentation.sharpening
        || (request.presentation.color == ColorRequirement::Srgb
            && artifact.variant.presentation.color != ColorState::Srgb)
        || !artifact_matches(artifact, request.artifact)
    {
        return None;
    }

    let detail_satisfied = request.detail.accepts(
        artifact.facts.detail.sampled_dimensions,
        artifact.facts.native_detail(),
    );
    if detail_satisfied {
        Some(Satisfaction::Satisfied)
    } else if request.allow_interim {
        Some(Satisfaction::Interim)
    } else {
        None
    }
}

fn artifact_matches(artifact: &MediaArtifact, requirement: ArtifactRequirement) -> bool {
    match requirement {
        ArtifactRequirement::BoundedThumbnail { target } => {
            artifact.variant.target == target
                && crate::delivery::THUMBNAIL_LIMITS
                    .accepts(artifact.facts.display_dimensions.0, artifact.byte_size)
        }
        ArtifactRequirement::ExactVariant { origin, target } => {
            artifact.facts.source.origin == origin && artifact.variant.target == target
        }
        ArtifactRequirement::AnyDisplay => true,
        ArtifactRequirement::Exact(expected) => artifact.facts.source.origin == expected,
    }
}

pub(crate) fn candidate_rank(
    artifact: &MediaArtifact,
    satisfaction: Satisfaction,
) -> (u8, u64, u64) {
    let satisfaction_rank = match satisfaction {
        Satisfaction::Satisfied => 0,
        Satisfaction::Interim => 1,
    };
    let pixels = u64::from(artifact.facts.display_dimensions.0.width)
        * u64::from(artifact.facts.display_dimensions.0.height);
    let location_rank = match artifact.location {
        ArtifactLocation::Managed(_) => 0,
        ArtifactLocation::Original(_) => 1,
    };
    (satisfaction_rank, pixels, location_rank)
}

fn long_edge(dimensions: PixelDimensions) -> u32 {
    dimensions.width.max(dimensions.height)
}

#[cfg(unix)]
fn update_path(hasher: &mut Sha256, path: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt;
    hasher.update(path.as_os_str().as_bytes());
    hasher.update([0]);
}

#[cfg(windows)]
fn update_path(hasher: &mut Sha256, path: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt;
    for code_unit in path.as_os_str().encode_wide() {
        hasher.update(code_unit.to_le_bytes());
    }
    hasher.update([0, 0]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn artifact(origin: ImageOrigin, dimensions: PixelDimensions) -> MediaArtifact {
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("source.jpg");
        fs::write(&path, b"source").unwrap();
        let source_revision = SourceRevision::observe(&path).unwrap();
        let mut facts = crate::media_source::test_facts(origin, dimensions, false);
        crate::media_source::bind_facts(&mut facts, &source_revision).unwrap();
        MediaArtifact {
            artifact_id: "artifact".into(),
            source_revision,
            variant: VariantIdentity {
                presentation: ArtifactPresentation {
                    geometry: None,
                    orientation: OrientationState::Applied,
                    color: ColorState::Srgb,
                    sharpening: SharpeningState::None,
                },
                policy_revision: MEDIA_CACHE_POLICY_REVISION,
                target: "test".into(),
            },
            facts,
            byte_size: 1,
            media_type: "image/jpeg".into(),
            location: ArtifactLocation::Managed(PathBuf::from("artifact.jpg")),
        }
    }

    fn request(artifact: &MediaArtifact, detail: DetailRequirement) -> CacheRequest {
        CacheRequest {
            source_revision: artifact.source_revision.clone(),
            detail,
            artifact: ArtifactRequirement::AnyDisplay,
            presentation: PresentationRequirement {
                orientation: OrientationRequirement::Exact(OrientationState::Applied),
                color: ColorRequirement::Srgb,
                sharpening: SharpeningState::None,
            },
            policy_revision: MEDIA_CACHE_POLICY_REVISION,
            allow_interim: true,
        }
    }

    #[test]
    fn high_resolution_satisfies_lower_request_but_not_the_reverse() {
        let large = artifact(
            ImageOrigin::PrimaryImage,
            PixelDimensions {
                width: 4096,
                height: 2731,
            },
        );
        let small = artifact(
            ImageOrigin::PrimaryImage,
            PixelDimensions {
                width: 160,
                height: 120,
            },
        );
        assert_eq!(
            satisfies(
                &large,
                &request(&large, DetailRequirement::Display { min_long_edge: 512 })
            ),
            Some(Satisfaction::Satisfied)
        );
        assert_eq!(
            satisfies(
                &small,
                &request(
                    &small,
                    DetailRequirement::Display {
                        min_long_edge: 4096
                    }
                )
            ),
            Some(Satisfaction::Interim)
        );
    }

    #[test]
    fn thumbnail_delivery_rejects_large_native_artifacts_and_old_policy() {
        let mut candidate = artifact(
            ImageOrigin::PrimaryImage,
            PixelDimensions {
                width: 4096,
                height: 2731,
            },
        );
        candidate.facts.detail.sampling = oxy_domain::Sampling::Native;
        let mut bounded = request(
            &candidate,
            DetailRequirement::Display { min_long_edge: 512 },
        );
        bounded.artifact = ArtifactRequirement::BoundedThumbnail { target: "test" };
        assert_eq!(satisfies(&candidate, &bounded), None);
        candidate.facts.display_dimensions = oxy_domain::DisplayDimensions(PixelDimensions {
            width: 512,
            height: 341,
        });
        candidate.facts.encoded_dimensions =
            oxy_domain::EncodedDimensions(candidate.facts.display_dimensions.0);
        candidate.facts.detail.sampled_dimensions = candidate.facts.display_dimensions;
        assert_eq!(
            satisfies(&candidate, &bounded),
            Some(Satisfaction::Satisfied)
        );
        candidate.byte_size = crate::delivery::THUMBNAIL_BYTES + 1;
        assert_eq!(satisfies(&candidate, &bounded), None);
        candidate.byte_size = 1000;
        candidate.variant.target = "old-thumbnail".into();
        assert_eq!(satisfies(&candidate, &bounded), None);
        bounded.artifact = ArtifactRequirement::AnyDisplay;
        assert_eq!(
            satisfies(&candidate, &bounded),
            Some(Satisfaction::Satisfied)
        );
    }

    #[test]
    fn incompatible_presentation_and_representation_never_match() {
        let developed = artifact(
            ImageOrigin::RawSensor,
            PixelDimensions {
                width: 7008,
                height: 4672,
            },
        );
        let mut request = request(&developed, DetailRequirement::NativeDetail);
        request.artifact = ArtifactRequirement::Exact(ImageOrigin::EmbeddedPreview);
        assert_eq!(satisfies(&developed, &request), None);
        request.artifact = ArtifactRequirement::AnyDisplay;
        request.presentation.sharpening = SharpeningState::Display;
        assert_eq!(satisfies(&developed, &request), None);
    }

    #[test]
    fn exact_variant_and_minimum_edges_are_independent_requirements() {
        let mut candidate = artifact(
            ImageOrigin::EmbeddedPreview,
            PixelDimensions {
                width: 600,
                height: 400,
            },
        );
        let mut request = request(
            &candidate,
            DetailRequirement::MinimumDimensions {
                min_long_edge: 600,
                min_short_edge: 400,
            },
        );
        request.artifact = ArtifactRequirement::ExactVariant {
            origin: ImageOrigin::EmbeddedPreview,
            target: "selected-variant",
        };
        assert_eq!(satisfies(&candidate, &request), None);
        candidate.variant.target = "selected-variant".into();
        assert_eq!(
            satisfies(&candidate, &request),
            Some(Satisfaction::Satisfied)
        );
        candidate.facts.display_dimensions = oxy_domain::DisplayDimensions(PixelDimensions {
            width: 400,
            height: 600,
        });
        candidate.facts.encoded_dimensions =
            oxy_domain::EncodedDimensions(candidate.facts.display_dimensions.0);
        candidate.facts.detail.sampled_dimensions = candidate.facts.display_dimensions;
        assert_eq!(
            satisfies(&candidate, &request),
            Some(Satisfaction::Satisfied)
        );
        candidate.facts.display_dimensions.0.width = 399;
        candidate.facts.encoded_dimensions.0.width = 399;
        candidate.facts.detail.sampled_dimensions.0.width = 399;
        candidate.facts.detail.sampling = oxy_domain::Sampling::Native;
        assert_eq!(satisfies(&candidate, &request), Some(Satisfaction::Interim));
        request.allow_interim = false;
        assert_eq!(satisfies(&candidate, &request), None);
        candidate.facts.display_dimensions = oxy_domain::DisplayDimensions(PixelDimensions {
            width: 400,
            height: 599,
        });
        candidate.facts.encoded_dimensions =
            oxy_domain::EncodedDimensions(candidate.facts.display_dimensions.0);
        candidate.facts.detail.sampled_dimensions = candidate.facts.display_dimensions;
        assert_eq!(satisfies(&candidate, &request), None);
        candidate.facts.display_dimensions.0.height = 600;
        candidate.facts.encoded_dimensions.0.height = 600;
        candidate.facts.detail.sampled_dimensions.0.height = 600;
        candidate.facts.source.origin = ImageOrigin::RawSensor;
        assert_eq!(satisfies(&candidate, &request), None);
    }

    #[test]
    fn downsampled_tiff_or_heif_artifact_is_only_interim_for_full() {
        let downsample = artifact(
            ImageOrigin::PrimaryImage,
            PixelDimensions {
                width: 4096,
                height: 2731,
            },
        );
        assert_eq!(
            satisfies(
                &downsample,
                &request(&downsample, DetailRequirement::NativeDetail)
            ),
            Some(Satisfaction::Interim)
        );
        let mut no_interim = request(&downsample, DetailRequirement::NativeDetail);
        no_interim.allow_interim = false;
        assert_eq!(satisfies(&downsample, &no_interim), None);
    }

    #[test]
    fn native_small_original_satisfies_large_display_without_upscaling() {
        let mut original = artifact(
            ImageOrigin::PrimaryImage,
            PixelDimensions {
                width: 800,
                height: 600,
            },
        );
        original.facts.detail.sampling = oxy_domain::Sampling::Native;
        assert_eq!(
            satisfies(
                &original,
                &request(
                    &original,
                    DetailRequirement::Display {
                        min_long_edge: 4096
                    }
                )
            ),
            Some(Satisfaction::Satisfied)
        );
    }
    #[test]
    fn partial_reference_coverage_is_not_a_whole_image_preview() {
        let mut cropped = artifact(ImageOrigin::PrimaryImage, (4096, 2731).into());
        cropped.facts.detail.region.width /= 2;
        let display = request(&cropped, DetailRequirement::Display { min_long_edge: 512 });
        assert!(cropped.facts.is_consistent());
        assert_eq!(satisfies(&cropped, &display), None);
    }
}
