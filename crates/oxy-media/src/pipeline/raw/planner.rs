//! RAW viewing policy translated into format-independent cache requirements.
use crate::{
    ImageDimensions,
    cache::{
        ArtifactRequirement, CacheRequest, DetailRequirement, ImageOrigin, OrientationRequirement,
        PresentationRequirement, SharpeningState,
    },
    pipeline::artifact::{ArtifactCache, applied_srgb_requirement},
};

use oxy_domain::RenderLevel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawBackend {
    #[cfg(target_os = "macos")]
    AppleCoreImage,
    #[cfg(target_os = "macos")]
    AppleImageIo,
    #[cfg(target_os = "windows")]
    WindowsWic,
    LibRawDevelopment,
}

pub(crate) fn plan_backends(level: RenderLevel) -> Vec<RawBackend> {
    #[cfg(target_os = "macos")]
    return match level {
        RenderLevel::Thumbnail => vec![
            RawBackend::AppleImageIo,
            RawBackend::AppleCoreImage,
            RawBackend::LibRawDevelopment,
        ],
        RenderLevel::Preview | RenderLevel::Full => vec![
            RawBackend::AppleCoreImage,
            RawBackend::AppleImageIo,
            RawBackend::LibRawDevelopment,
        ],
    };
    #[cfg(target_os = "windows")]
    if level == RenderLevel::Full {
        return vec![RawBackend::WindowsWic, RawBackend::LibRawDevelopment];
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        let _ = level;
        vec![RawBackend::LibRawDevelopment]
    }
}

pub(super) fn display_requirement() -> PresentationRequirement {
    PresentationRequirement {
        orientation: OrientationRequirement::DisplayCorrect,
        color: crate::cache::ColorRequirement::Any,
        sharpening: SharpeningState::None,
    }
}

pub(crate) fn preview_request(
    artifacts: &ArtifactCache,
    max_size: u32,
    allow_interim: bool,
) -> crate::cache::CacheRequest {
    artifacts.request(
        DetailRequirement::Display {
            min_long_edge: max_size,
        },
        ArtifactRequirement::AnyDisplay,
        display_requirement(),
        allow_interim,
    )
}

pub(super) const fn preview_max_size(level: RenderLevel) -> Option<u32> {
    match level {
        RenderLevel::Thumbnail => Some(512),
        RenderLevel::Preview => Some(4_096),
        RenderLevel::Full => None,
    }
}

// Records completed embedded selection, not satisfaction of a viewing level.
pub(super) const LARGEST_EMBEDDED_JPEG_TARGET: &str =
    "raw-largest-embedded-jpeg-v2-resolved-dimensions";

pub(super) fn largest_preview_request(artifacts: &ArtifactCache) -> CacheRequest {
    artifacts.request(
        DetailRequirement::Display { min_long_edge: 1 },
        ArtifactRequirement::ExactVariant {
            origin: ImageOrigin::EmbeddedPreview,
            target: LARGEST_EMBEDDED_JPEG_TARGET.into(),
        },
        display_requirement(),
        false,
    )
}

pub(super) struct RawFullPlan {
    pub embedded: CacheRequest,
    pub developed: CacheRequest,
    pub regenerate: bool,
}

impl RawFullPlan {
    pub fn new(artifacts: &ArtifactCache, source: ImageDimensions) -> Self {
        Self {
            regenerate: crate::raw_retry_revision(artifacts.source_revision_id()).is_some(),
            embedded: artifacts.request(
                camera_preview_detail(source),
                ArtifactRequirement::ExactVariant {
                    origin: ImageOrigin::EmbeddedPreview,
                    target: LARGEST_EMBEDDED_JPEG_TARGET.into(),
                },
                display_requirement(),
                false,
            ),
            developed: full_developed_request(artifacts),
        }
    }

    pub fn embedded_production(&self) -> CacheRequest {
        CacheRequest {
            allow_interim: true,
            ..self.embedded.clone()
        }
    }
}

pub(super) fn full_target(artifacts: &ArtifactCache) -> String {
    match crate::raw_retry_revision(artifacts.source_revision_id()) {
        Some(revision) => format!("{}:generation-{revision}", crate::policy::RAW_FULL),
        None => crate::policy::RAW_FULL.into(),
    }
}

pub(crate) fn full_developed_request(artifacts: &ArtifactCache) -> CacheRequest {
    artifacts.request(
        DetailRequirement::NativeDetail,
        ArtifactRequirement::ExactVariant {
            origin: ImageOrigin::RawSensor,
            target: full_target(artifacts),
        },
        applied_srgb_requirement(),
        false,
    )
}

pub(super) fn camera_preview_detail(source: ImageDimensions) -> DetailRequirement {
    // Preserve the current 90% requirement on BOTH display-space edges.
    // Round upward so fractional pixel thresholds cannot admit a smaller JPEG.
    let minimum = |edge: u32| (u64::from(edge) * 90).div_ceil(100) as u32;
    DetailRequirement::MinimumDimensions {
        min_long_edge: minimum(source.width.max(source.height)),
        min_short_edge: minimum(source.width.min(source.height)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        media_source::PixelDimensions,
        pipeline::artifact::{ArtifactPreparation, applied_srgb},
    };
    use image::{DynamicImage, ImageFormat};
    use oxy_domain::{MediaSatisfaction, PreviewKind, RenderLevel};
    use std::{io::Cursor, sync::Arc};

    #[cfg(target_os = "windows")]
    #[test]
    fn system_raw_precedes_libraw_only_for_full_development() {
        assert_eq!(
            plan_backends(RenderLevel::Full),
            vec![RawBackend::WindowsWic, RawBackend::LibRawDevelopment]
        );
        for level in [RenderLevel::Thumbnail, RenderLevel::Preview] {
            assert_eq!(plan_backends(level), vec![RawBackend::LibRawDevelopment]);
        }
    }

    #[test]
    fn full_has_no_bounded_preview_target() {
        assert_eq!(preview_max_size(RenderLevel::Thumbnail), Some(512));
        assert_eq!(preview_max_size(RenderLevel::Preview), Some(4_096));
        assert_eq!(preview_max_size(RenderLevel::Full), None);
    }

    #[test]
    fn camera_coverage_requires_both_edges_and_rounds_up_without_overflow() {
        let detail = camera_preview_detail(ImageDimensions {
            width: 60,
            height: 100,
        });
        for (width, height, accepted) in [
            (90, 54, true),
            (54, 90, true),
            (89, 60, false),
            (100, 53, false),
        ] {
            assert_eq!(
                detail.accepts(
                    oxy_domain::DisplayDimensions(PixelDimensions { width, height }),
                    false
                ),
                accepted
            );
        }
        let rounded = camera_preview_detail(ImageDimensions {
            width: 101,
            height: 61,
        });
        assert!(!rounded.accepts(
            oxy_domain::DisplayDimensions(PixelDimensions {
                width: 90,
                height: 55
            }),
            false
        ));
        assert!(rounded.accepts(
            oxy_domain::DisplayDimensions(PixelDimensions {
                width: 91,
                height: 55
            }),
            false
        ));
        assert!(
            camera_preview_detail(ImageDimensions {
                width: u32::MAX,
                height: u32::MAX
            })
            .accepts(
                oxy_domain::DisplayDimensions(PixelDimensions {
                    width: u32::MAX,
                    height: u32::MAX
                }),
                false
            )
        );
    }

    fn publish(
        artifacts: &ArtifactCache,
        request: &CacheRequest,
        width: u32,
        height: u32,
        developed: bool,
    ) -> oxy_domain::PreviewResult {
        let ArtifactPreparation::Generate { cache_generation } =
            artifacts.prepare(request, RenderLevel::Full).unwrap()
        else {
            panic!("expected a cold variant");
        };
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(width, height)
            .write_to(&mut bytes, ImageFormat::Jpeg)
            .unwrap();
        artifacts
            .publish(
                Arc::from(bytes.into_inner()),
                crate::media_source::test_facts(
                    if developed {
                        ImageOrigin::RawSensor
                    } else {
                        ImageOrigin::EmbeddedPreview
                    },
                    PixelDimensions { width, height },
                    developed,
                ),
                applied_srgb(),
                if developed {
                    full_target(artifacts)
                } else {
                    LARGEST_EMBEDDED_JPEG_TARGET.into()
                },
                RenderLevel::Full,
                cache_generation,
                request,
            )
            .unwrap()
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn regeneration_requires_new_full_identity_and_preserves_existing_display_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.raw");
        std::fs::write(&source, b"source revision").unwrap();
        let artifacts = ArtifactCache::new(&source, &directory.path().join("cache")).unwrap();
        let dimensions = ImageDimensions {
            width: 100,
            height: 60,
        };
        let previous = RawFullPlan::new(&artifacts, dimensions);
        publish(&artifacts, &previous.embedded_production(), 90, 54, false);
        publish(&artifacts, &previous.developed, 100, 60, true);
        assert!(!previous.regenerate);

        crate::request_raw_retry(&source).unwrap();
        let current = RawFullPlan::new(&artifacts, dimensions);
        assert!(current.regenerate);
        assert_ne!(current.developed.artifact, previous.developed.artifact);
        assert!(
            artifacts
                .lookup(&current.developed, RenderLevel::Full)
                .unwrap()
                .is_none()
        );
        assert!(
            artifacts
                .lookup(&previous.developed, RenderLevel::Full)
                .unwrap()
                .is_some()
        );
        assert!(
            artifacts
                .lookup(&current.embedded, RenderLevel::Full)
                .unwrap()
                .is_some()
        );
        assert!(
            artifacts
                .lookup(
                    &preview_request(&artifacts, 32, false),
                    RenderLevel::Thumbnail
                )
                .unwrap()
                .is_some()
        );
        publish(&artifacts, &current.developed, 100, 60, true);
        assert!(
            artifacts
                .lookup(&current.developed, RenderLevel::Full)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn full_plan_restores_either_qualified_camera_jpeg_or_development() {
        for near_native in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("source.raw");
            std::fs::write(&source, b"source revision").unwrap();
            let cache_dir = directory.path().join("cache");
            let artifacts = ArtifactCache::new(&source, &cache_dir).unwrap();
            let dimensions = ImageDimensions {
                width: 100,
                height: 60,
            };
            let plan = RawFullPlan::new(&artifacts, dimensions);
            let (width, height) = if near_native { (90, 54) } else { (32, 20) };
            let selected = publish(
                &artifacts,
                &plan.embedded_production(),
                width,
                height,
                false,
            );
            assert_eq!(
                selected.satisfaction,
                Some(if near_native {
                    MediaSatisfaction::Satisfied
                } else {
                    MediaSatisfaction::Interim
                })
            );
            if !near_native {
                assert!(
                    artifacts
                        .lookup(&plan.embedded, RenderLevel::Full)
                        .unwrap()
                        .is_none()
                );
                let developed = publish(&artifacts, &plan.developed, 100, 60, true);
                assert_eq!(developed.satisfaction, Some(MediaSatisfaction::Satisfied));
            }
            drop(artifacts);
            let restored = ArtifactCache::new(&source, &cache_dir).unwrap();
            let plan = RawFullPlan::new(&restored, dimensions);
            let ArtifactPreparation::Cached(full) = restored
                .prepare_candidates(
                    &plan.embedded,
                    std::slice::from_ref(&plan.developed),
                    RenderLevel::Full,
                )
                .unwrap()
            else {
                panic!("full result must survive reopening the disk cache");
            };
            assert_eq!(full.satisfaction, Some(MediaSatisfaction::Satisfied));
            assert_eq!(
                full.kind,
                if near_native {
                    PreviewKind::Embedded
                } else {
                    PreviewKind::Developed
                }
            );
        }
    }

    #[test]
    fn small_largest_camera_jpeg_satisfies_preview_but_not_full() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.raw");
        std::fs::write(&source, b"source revision").unwrap();
        let cache_dir = directory.path().join("cache");
        let artifacts = ArtifactCache::new(&source, &cache_dir).unwrap();
        let plan = RawFullPlan::new(
            &artifacts,
            ImageDimensions {
                width: 100,
                height: 60,
            },
        );
        publish(&artifacts, &plan.embedded_production(), 32, 20, false);
        assert!(
            artifacts
                .lookup(&plan.embedded, RenderLevel::Full)
                .unwrap()
                .is_none()
        );
        let preview = super::super::preview_with_priority(
            &source,
            &cache_dir,
            4096,
            crate::decode_control::DecodePriority::Foreground,
            false,
            &oxy_runtime::CancellationToken::default(),
        )
        .unwrap();
        assert_eq!((preview.width, preview.height), (32, 20));
        assert_eq!(preview.satisfaction, Some(MediaSatisfaction::Satisfied));
        assert_eq!(preview.kind, PreviewKind::Embedded);
    }
}
