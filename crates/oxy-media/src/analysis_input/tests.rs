use super::*;
use crate::SharpeningState;
use crate::pipeline::input::qualified;
use crate::{DiskMediaCache, MEDIA_CACHE_POLICY_REVISION, MediaCache};
use oxy_domain::{DisplayDimensions, EncodedDimensions, PixelDimensions};
fn cached(
    cache: &dyn MediaCache,
    source: &SourceRevision,
    dimensions: PixelDimensions,
    target: u32,
    token: &CancellationToken,
) -> Result<Option<AnalysisInput>, MediaError> {
    crate::pipeline::input::cached(
        cache,
        source,
        MediaRequest::full_frame(dimensions, target),
        token,
    )
    .map(|input| input.map(super::into_analysis))
}
use image::{DynamicImage, RgbImage, codecs::jpeg::JpegEncoder};

const REQUIREMENT: AnalysisRequirement = AnalysisRequirement {
    minimum_source_long_edge: 112,
    target_long_edge: 1600,
};

#[test]
fn model_adapter_cannot_own_format_selection_or_decoders() {
    let adapter = include_str!("../analysis_input.rs");
    for forbidden in [
        "AssetKind::",
        "backends::",
        "cache.lookup",
        "ImageReader",
        "libraw",
        "libjpeg",
    ] {
        assert!(
            !adapter.contains(forbidden),
            "model adapter owns media behavior: {forbidden}"
        );
    }
}

#[test]
fn raster_display_and_pixel_delivery_preserve_identical_alpha_and_orientation() {
    for (extension, kind) in [
        ("png", AssetKind::Png),
        ("webp", AssetKind::Webp),
        ("tiff", AssetKind::Tiff),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("source.{extension}"));
        image::RgbaImage::from_fn(900, 600, |x, y| {
            image::Rgba([x as u8, y as u8, 80, if x < 450 { 40 } else { 255 }])
        })
        .save(&path)
        .unwrap();
        let display = crate::preview(
            &path,
            directory.path(),
            oxy_domain::RenderLevel::Thumbnail,
            oxy_domain::PreviewPriority::Visible,
            kind,
            &CancellationToken::default(),
        )
        .unwrap();
        let source = SourceRevision::observe(&path).unwrap();
        let input = crate::pipeline::input::prepare_uncached(
            &path,
            kind,
            &source,
            MediaRequest::full_frame((900, 600).into(), 512),
            &CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(
            image::open(display.path).unwrap().to_rgba8(),
            input.image.to_rgba8()
        );
        assert_eq!(
            display.image_facts.unwrap().detail.sampled_dimensions,
            input.facts.detail.sampled_dimensions
        );
    }
}

#[test]
fn raster_formats_preserve_channels_alpha_and_do_not_enlarge() {
    for (extension, kind) in [
        ("png", AssetKind::Png),
        ("webp", AssetKind::Webp),
        ("tiff", AssetKind::Tiff),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("image.{extension}"));
        let pixels = image::RgbaImage::from_fn(320, 240, |x, _| {
            if x < 160 {
                image::Rgba([31, 73, 149, 255])
            } else {
                image::Rgba([90, 0, 0, 0])
            }
        });
        pixels.save(&path).unwrap();
        let source = SourceRevision::observe(&path).unwrap();
        let input = prepare_analysis_input(
            &path,
            kind,
            &source,
            REQUIREMENT,
            &CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(input.pixels.dimensions(), (320, 240));
        assert_eq!(input.pixels.get_pixel(0, 0).0, [31, 73, 149]);
        assert_eq!(input.pixels.get_pixel(319, 0).0, [255, 255, 255]);
        assert!(input.facts.is_consistent());
        assert_eq!(input.facts.source.revision_id, source.revision_id);
    }
}

#[test]
fn rejects_invalid_request_and_cancellation_before_io() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("gone.jpg");
    std::fs::write(&path, b"not an image").unwrap();
    let source = SourceRevision::observe(&path).unwrap();
    let token = CancellationToken::default();
    assert!(matches!(
        prepare_analysis_input(
            &path,
            AssetKind::Jpeg,
            &source,
            AnalysisRequirement {
                minimum_source_long_edge: 0,
                ..REQUIREMENT
            },
            &token
        ),
        Err(MediaError::InvalidAnalysisRequirement)
    ));
    token.cancel();
    assert!(matches!(
        prepare_analysis_input(&path, AssetKind::Jpeg, &source, REQUIREMENT, &token),
        Err(MediaError::Cancelled)
    ));
}

#[test]
fn embedded_detail_requires_both_axes_and_complete_coverage() {
    let reference = PixelDimensions {
        width: 6000,
        height: 4000,
    };
    let mut facts = crate::media_source::source_facts(
        oxy_domain::ImageOrigin::EmbeddedPreview,
        "camera".into(),
        EncodedDimensions((1600, 120).into()),
        1,
        DisplayDimensions(reference),
    );
    assert!(!qualified(&facts, reference, 1600));
    facts = crate::media_source::source_facts(
        oxy_domain::ImageOrigin::EmbeddedPreview,
        "camera".into(),
        EncodedDimensions((1600, 1067).into()),
        1,
        DisplayDimensions(reference),
    );
    assert!(qualified(&facts, reference, 1600));
    // Camera crop versus sensor border must not force CR3 development.
    assert!(qualified(&facts, (6022, 4024).into(), 1600));
    let padded = crate::media_source::source_facts(
        oxy_domain::ImageOrigin::EmbeddedPreview,
        "hif-aux".into(),
        EncodedDimensions((1046, 1600).into()),
        1,
        DisplayDimensions(reference),
    );
    assert!(!qualified(&padded, reference, 1600));
    facts.detail.region.width = 3000;
    assert!(!qualified(&facts, reference, 1600));
}

#[test]
fn cache_reuses_only_qualified_unsharpened_current_artifacts() {
    use crate::{ArtifactPresentation, OrientationState, PendingArtifact, VariantIdentity};
    use oxy_domain::ImageOrigin;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.raw");
    std::fs::write(&path, b"source identity").unwrap();
    let source = SourceRevision::observe(&path).unwrap();
    let cache = DiskMediaCache::new(directory.path(), 8).unwrap();
    let publish = |size: (u32, u32), sharpening| {
        let mut bytes = Vec::new();
        JpegEncoder::new(&mut bytes)
            .encode_image(&DynamicImage::new_rgb8(size.0, size.1))
            .unwrap();
        let mut facts = crate::media_source::source_facts(
            ImageOrigin::EmbeddedPreview,
            "camera-jpeg".into(),
            EncodedDimensions(size.into()),
            1,
            DisplayDimensions((6000, 4000).into()),
        );
        crate::media_source::bind_facts(&mut facts, &source).unwrap();
        cache
            .publish(PendingArtifact {
                source_revision: source.clone(),
                variant: VariantIdentity {
                    presentation: ArtifactPresentation {
                        geometry: None,
                        orientation: OrientationState::Applied,
                        color: CacheColorState::EmbeddedOrUnknown,
                        sharpening,
                    },
                    policy_revision: MEDIA_CACHE_POLICY_REVISION,
                    target: format!("{size:?}-{sharpening:?}"),
                },
                facts,
                media_type: "image/jpeg".into(),
                extension: "jpg".into(),
                bytes: bytes.into(),
                cache_generation: cache.generation().unwrap(),
            })
            .unwrap()
    };
    let _small = publish((160, 120), SharpeningState::None);
    let _display = publish((1800, 1200), SharpeningState::Display);
    let token = CancellationToken::default();
    assert!(
        cached(&cache, &source, (6000, 4000).into(), 1600, &token)
            .unwrap()
            .is_none()
    );
    let _large = publish((1800, 1200), SharpeningState::None);
    let input = cached(&cache, &source, (6000, 4000).into(), 1600, &token)
        .unwrap()
        .unwrap();
    assert!(input.cache_hit);
    assert_eq!(input.pixels.dimensions(), (1600, 1067));
    assert!(qualified(&input.facts, (6000, 4000).into(), 1600));
    std::fs::write(&path, b"changed source identity").unwrap();
    let changed = SourceRevision::observe(&path).unwrap();
    assert!(
        cached(&cache, &changed, (6000, 4000).into(), 1600, &token)
            .unwrap()
            .is_none()
    );
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires bundled FFmpeg and the real Sony HIF fixture"]
fn hif_frame_selection_matches_display_and_preserves_primary_pixels() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/DSC00449.HIF");
    let token = CancellationToken::default();
    let dimensions = crate::dimensions(&path, AssetKind::Heif).unwrap();
    let frame = crate::pipeline::heif::artifact::decode_frame(
        &path,
        crate::request::MediaRequest::full_frame(dimensions, 1600),
        &token,
    )
    .unwrap();
    let input = prepare_analysis_input(
        &path,
        AssetKind::Heif,
        &SourceRevision::observe(&path).unwrap(),
        REQUIREMENT,
        &token,
    )
    .unwrap();
    assert_eq!(input.pixels.dimensions(), (1067, 1600));
    assert_eq!(input.pixels, frame.image.to_rgb8());
    assert_eq!(
        input.facts.source.origin,
        oxy_domain::ImageOrigin::PrimaryImage
    );
    assert!(
        input
            .facts
            .processing
            .iter()
            .any(|operation| matches!(operation,
        oxy_domain::ImageOperation::Decode { backend } if backend == "FFmpeg"))
    );
    assert!(
        !input
            .facts
            .processing
            .iter()
            .any(|operation| matches!(operation, oxy_domain::ImageOperation::Encode { .. }))
    );
}

#[test]
#[ignore = "real release fixture measurement; set OXY_ANALYSIS_FIXTURES to a directory"]
fn real_format_inputs() {
    let root = std::env::var_os("OXY_ANALYSIS_FIXTURES").expect("fixture directory required");
    let cache = tempfile::tempdir().unwrap();
    let service = AnalysisInputService::new(cache.path()).unwrap();
    let mut count = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let kind = match path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "jpg" | "jpeg" => AssetKind::Jpeg,
            "hif" | "heif" | "heic" => AssetKind::Heif,
            "arw" | "cr3" | "rw2" | "nef" | "dng" | "cr2" | "raf" | "orf" => AssetKind::Raw,
            "png" => AssetKind::Png,
            "webp" => AssetKind::Webp,
            "tif" | "tiff" => AssetKind::Tiff,
            _ => continue,
        };
        let revision = SourceRevision::observe(&path).unwrap();
        for attempt in 0..2 {
            let start = std::time::Instant::now();
            let input = service
                .prepare(
                    &path,
                    kind,
                    &revision,
                    REQUIREMENT,
                    &CancellationToken::default(),
                )
                .unwrap();
            println!(
                "{} attempt={attempt} {:?} {:?} {}ms cache={} operations={:?}",
                path.file_name().unwrap().to_string_lossy(),
                input.pixels.dimensions(),
                input.facts.source.origin,
                start.elapsed().as_millis(),
                input.cache_hit,
                input.facts.processing
            );
            assert!(input.facts.is_consistent());
            assert!(input.pixels.width().max(input.pixels.height()) <= 1600);
        }
        if matches!(kind, AssetKind::Raw | AssetKind::Heif) {
            // Seed through the real display artifact producer, not an analysis
            // fixture written directly into a cache manifest.
            let token = CancellationToken::default();
            if kind == AssetKind::Heif {
                crate::pipeline::heif::artifact::full(&path, cache.path(), &token).unwrap();
            } else {
                crate::preview(
                    &path,
                    cache.path(),
                    oxy_domain::RenderLevel::Preview,
                    oxy_domain::PreviewPriority::Preload,
                    kind,
                    &token,
                )
                .unwrap();
            }
            let start = std::time::Instant::now();
            let input = service
                .prepare(&path, kind, &revision, REQUIREMENT, &token)
                .unwrap();
            println!(
                "{} display-cache {:?} {}ms hit={}",
                path.file_name().unwrap().to_string_lossy(),
                input.pixels.dimensions(),
                start.elapsed().as_millis(),
                input.cache_hit
            );
            assert!(
                input.cache_hit,
                "real qualified display artifact must be reused"
            );
        }
        count += 1;
    }
    assert!(count > 0, "no real fixtures tested");
}

#[test]
fn jpeg_input_scales_and_rejects_stale_or_insufficient_sources() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("photo.jpg");
    let image = DynamicImage::ImageRgb8(RgbImage::new(1600, 1000));
    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, 85)
        .encode_image(&image)
        .unwrap();
    std::fs::write(&path, encoded).unwrap();
    let revision = SourceRevision::observe(&path).unwrap();
    let cancellation = CancellationToken::default();
    let result = prepare_analysis_input(
        &path,
        AssetKind::Jpeg,
        &revision,
        AnalysisRequirement {
            minimum_source_long_edge: 100,
            target_long_edge: 800,
        },
        &cancellation,
    )
    .unwrap();
    assert_eq!(result.pixels.dimensions(), (800, 500));
    assert_eq!(result.source_revision, revision);
    assert!(matches!(
        prepare_analysis_input(
            &path,
            AssetKind::Jpeg,
            &revision,
            AnalysisRequirement {
                minimum_source_long_edge: 2000,
                target_long_edge: 2000,
            },
            &cancellation,
        ),
        Err(MediaError::InsufficientAnalysisDetail { .. })
    ));
    std::fs::write(&path, b"replacement").unwrap();
    assert!(matches!(
        prepare_analysis_input(
            &path,
            AssetKind::Jpeg,
            &revision,
            AnalysisRequirement {
                minimum_source_long_edge: 100,
                target_long_edge: 2000,
            },
            &cancellation,
        ),
        Err(MediaError::StaleSourceRevision)
    ));
}

#[test]
fn jpeg_input_applies_exif_orientation_before_returning_pixels() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("rotated.jpg");
    let image = DynamicImage::ImageRgb8(RgbImage::new(80, 40));
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 85)
        .encode_image(&image)
        .unwrap();
    // Exif APP1 with a little-endian Orientation=6 IFD entry.
    let exif: [u8; 32] = [
        b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0,
        0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut encoded = vec![0xff, 0xd8, 0xff, 0xe1, 0, 34];
    encoded.extend_from_slice(&exif);
    encoded.extend_from_slice(&jpeg[2..]);
    std::fs::write(&path, encoded).unwrap();
    let revision = SourceRevision::observe(&path).unwrap();
    let result = prepare_analysis_input(
        &path,
        AssetKind::Jpeg,
        &revision,
        AnalysisRequirement {
            minimum_source_long_edge: 80,
            target_long_edge: 80,
        },
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(
        result.facts.source.encoded_dimensions.unwrap().0,
        PixelDimensions {
            width: 80,
            height: 40
        }
    );
    assert_eq!(result.pixels.dimensions(), (40, 80));
}
