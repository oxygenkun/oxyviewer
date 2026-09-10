use crate::{ImageDimensions, MediaError};
use oxy_fs::observe_file;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub const MEDIA_CACHE_POLICY_REVISION: u32 = 1;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayDimensions {
    pub width: u32,
    pub height: u32,
}

impl From<(u32, u32)> for DisplayDimensions {
    fn from((width, height): (u32, u32)) -> Self {
        Self { width, height }
    }
}

impl From<ImageDimensions> for DisplayDimensions {
    fn from(value: ImageDimensions) -> Self {
        Self {
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactRepresentation {
    Original,
    Embedded,
    Decoded,
    Developed,
    System,
}

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
    Native {
        source: DisplayDimensions,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepresentationRequirement {
    AnyDisplay,
    Exact(ArtifactRepresentation),
    RawNative { allow_camera_preview: bool },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantIdentity {
    pub representation: ArtifactRepresentation,
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
    pub actual_dimensions: DisplayDimensions,
    pub native_detail: bool,
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
    pub representation: RepresentationRequirement,
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
    if artifact.source_revision != request.source_revision
        || artifact.variant.policy_revision != request.policy_revision
        || (matches!(
            request.presentation.orientation,
            OrientationRequirement::Exact(expected)
                if artifact.variant.presentation.orientation != expected
        ))
        || artifact.variant.presentation.sharpening != request.presentation.sharpening
        || (request.presentation.color == ColorRequirement::Srgb
            && artifact.variant.presentation.color != ColorState::Srgb)
        || !representation_matches(artifact, request.representation)
    {
        return None;
    }

    let detail_satisfied = match request.detail {
        DetailRequirement::Display { min_long_edge } => {
            artifact.native_detail || long_edge(artifact.actual_dimensions) >= min_long_edge
        }
        DetailRequirement::NativeDetail => artifact.native_detail,
        DetailRequirement::Native { source } => {
            artifact.native_detail
                || (matches!(
                    request.representation,
                    RepresentationRequirement::RawNative {
                        allow_camera_preview: true
                    }
                ) && artifact.variant.representation == ArtifactRepresentation::Embedded
                    && covers_percent(artifact.actual_dimensions, source, 90))
        }
    };
    if detail_satisfied {
        Some(Satisfaction::Satisfied)
    } else if request.allow_interim {
        Some(Satisfaction::Interim)
    } else {
        None
    }
}

fn representation_matches(
    artifact: &MediaArtifact,
    requirement: RepresentationRequirement,
) -> bool {
    match requirement {
        RepresentationRequirement::AnyDisplay => true,
        RepresentationRequirement::Exact(expected) => artifact.variant.representation == expected,
        RepresentationRequirement::RawNative {
            allow_camera_preview,
        } => {
            artifact.variant.representation == ArtifactRepresentation::Developed
                || (allow_camera_preview
                    && artifact.variant.representation == ArtifactRepresentation::Embedded)
        }
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
    let pixels =
        u64::from(artifact.actual_dimensions.width) * u64::from(artifact.actual_dimensions.height);
    let location_rank = match artifact.location {
        ArtifactLocation::Managed(_) => 0,
        ArtifactLocation::Original(_) => 1,
    };
    (satisfaction_rank, pixels, location_rank)
}

fn long_edge(dimensions: DisplayDimensions) -> u32 {
    dimensions.width.max(dimensions.height)
}

fn covers_percent(candidate: DisplayDimensions, source: DisplayDimensions, percent: u64) -> bool {
    let mut candidate_edges = [candidate.width, candidate.height];
    let mut source_edges = [source.width, source.height];
    candidate_edges.sort_unstable();
    source_edges.sort_unstable();
    candidate_edges
        .into_iter()
        .zip(source_edges)
        .all(|(candidate, source)| u64::from(candidate) * 100 >= u64::from(source) * percent)
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

    fn artifact(
        representation: ArtifactRepresentation,
        dimensions: DisplayDimensions,
    ) -> MediaArtifact {
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("source.jpg");
        fs::write(&path, b"source").unwrap();
        let source_revision = SourceRevision::observe(&path).unwrap();
        MediaArtifact {
            artifact_id: "artifact".into(),
            source_revision,
            variant: VariantIdentity {
                representation,
                presentation: ArtifactPresentation {
                    geometry: None,
                    orientation: OrientationState::Applied,
                    color: ColorState::Srgb,
                    sharpening: SharpeningState::None,
                },
                policy_revision: MEDIA_CACHE_POLICY_REVISION,
                target: "test".into(),
            },
            actual_dimensions: dimensions,
            native_detail: false,
            byte_size: 1,
            media_type: "image/jpeg".into(),
            location: ArtifactLocation::Managed(PathBuf::from("artifact.jpg")),
        }
    }

    fn request(artifact: &MediaArtifact, detail: DetailRequirement) -> CacheRequest {
        CacheRequest {
            source_revision: artifact.source_revision.clone(),
            detail,
            representation: RepresentationRequirement::AnyDisplay,
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
            ArtifactRepresentation::Decoded,
            DisplayDimensions {
                width: 4096,
                height: 2731,
            },
        );
        let small = artifact(
            ArtifactRepresentation::Decoded,
            DisplayDimensions {
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
    fn incompatible_presentation_and_representation_never_match() {
        let developed = artifact(
            ArtifactRepresentation::Developed,
            DisplayDimensions {
                width: 7008,
                height: 4672,
            },
        );
        let mut request = request(
            &developed,
            DetailRequirement::Native {
                source: developed.actual_dimensions,
            },
        );
        request.representation = RepresentationRequirement::Exact(ArtifactRepresentation::Embedded);
        assert_eq!(satisfies(&developed, &request), None);
        request.representation = RepresentationRequirement::AnyDisplay;
        request.presentation.sharpening = SharpeningState::Display;
        assert_eq!(satisfies(&developed, &request), None);
    }

    #[test]
    fn raw_camera_preview_requires_explicit_policy_and_near_native_coverage() {
        let embedded = artifact(
            ArtifactRepresentation::Embedded,
            DisplayDimensions {
                width: 6192,
                height: 4128,
            },
        );
        let source = DisplayDimensions {
            width: 6240,
            height: 4160,
        };
        let mut request = request(&embedded, DetailRequirement::Native { source });
        request.representation = RepresentationRequirement::RawNative {
            allow_camera_preview: true,
        };
        assert_eq!(
            satisfies(&embedded, &request),
            Some(Satisfaction::Satisfied)
        );
        request.representation = RepresentationRequirement::RawNative {
            allow_camera_preview: false,
        };
        assert_eq!(satisfies(&embedded, &request), None);
    }

    #[test]
    fn downsampled_tiff_or_heif_artifact_is_only_interim_for_full() {
        let downsample = artifact(
            ArtifactRepresentation::System,
            DisplayDimensions {
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
            ArtifactRepresentation::Original,
            DisplayDimensions {
                width: 800,
                height: 600,
            },
        );
        original.native_detail = true;
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
}
