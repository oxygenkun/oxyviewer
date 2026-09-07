use super::*;
#[cfg(target_os = "macos")]
use crate::backends::{apple_core_image, apple_image_io};
use crate::{
    backends::{libheif, libraw},
    cache::{preview_cache_key, write_jpeg_atomically},
    media_source::preview_result,
    pipeline::{
        heif_preview::{HEIF_CACHE_VERSION, heif_full, larger_cached_preview},
        raw::{self, RawBackend as PlannedRawBackend, covers_source as covers_raw_source},
        system::preview as system_preview,
    },
    presentation::{HEIF_DECODED_JPEG, RAW_DEVELOPED_JPEG},
};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Rgb, RgbImage};
use oxy_domain::{PreviewKind, RenderLevel};
use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn raw_preview(
    path: &Path,
    cache_dir: &Path,
    max_size: u32,
) -> Result<oxy_domain::PreviewResult, MediaError> {
    raw::preview_with_priority(path, cache_dir, max_size, DecodePriority::Background)
}

fn workspace_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.is_absolute() {
        path.to_owned()
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
    }
}

#[test]
fn heif_session_cache_is_lookup_only_and_comes_from_the_source_heif() {
    let directory = tempfile::tempdir().unwrap();
    let Some(source) = crate::sony_hif_fixture() else {
        eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
        return;
    };
    let cache = directory.path().join("previews");

    assert!(cached_heif_session(&source, &cache).unwrap().is_none());

    cache_heif_source_jpeg(&source, &cache).unwrap();
    let cached = cached_heif_session(&source, &cache)
        .unwrap()
        .expect("source HEIF should have a full JPEG cache");
    assert!(cached.width >= 4_672);
    assert!(cached.height >= 4_672);
    assert!(cached.path.is_file());
}

#[test]
fn sony_hif_thumbnail_and_preview_levels_share_the_160_artifact() {
    let Some(path) = crate::sony_hif_fixture() else {
        eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
        return;
    };
    let cache = tempfile::tempdir().unwrap();

    let thumbnail = preview(
        &path,
        cache.path(),
        RenderLevel::Thumbnail,
        DecodePriority::Visible,
        oxy_domain::AssetKind::Heif,
    )
    .unwrap();
    let loupe_base = preview(
        &path,
        cache.path(),
        RenderLevel::Preview,
        DecodePriority::Foreground,
        oxy_domain::AssetKind::Heif,
    )
    .unwrap();

    assert_eq!((thumbnail.width, thumbnail.height), (120, 160));
    assert_eq!(thumbnail.kind, PreviewKind::Embedded);
    assert_eq!(loupe_base.kind, PreviewKind::Embedded);
    assert_eq!(loupe_base.path, thumbnail.path);
    assert_eq!(thumbnail.render_level, Some(RenderLevel::Thumbnail));
    assert_eq!(loupe_base.render_level, Some(RenderLevel::Preview));
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 1);
}

#[test]
fn heif_without_identified_fast_representation_uses_semantic_preview_size() {
    let Some(source) = crate::sony_hif_fixture() else {
        eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("generic.heif");
    let mut bytes = fs::read(source).unwrap();
    assert_eq!(&bytes[36..40], b"SHIF");
    bytes[36..40].copy_from_slice(b"zzzz");
    fs::write(&path, bytes).unwrap();

    let cache = directory.path().join("cache");
    let thumbnail = preview(
        &path,
        &cache,
        RenderLevel::Thumbnail,
        DecodePriority::Visible,
        oxy_domain::AssetKind::Heif,
    )
    .unwrap();
    let fit = preview(
        &path,
        &cache,
        RenderLevel::Preview,
        DecodePriority::Foreground,
        oxy_domain::AssetKind::Heif,
    )
    .unwrap();

    assert_eq!(thumbnail.kind, PreviewKind::Decoded);
    assert_eq!(fit.kind, PreviewKind::Decoded);
    assert!(thumbnail.width.max(thumbnail.height) > 160);
    assert!(fit.width.max(fit.height) > thumbnail.width.max(thumbnail.height));
    assert_ne!(thumbnail.path, fit.path);
}

#[test]
fn larger_cached_preview_satisfies_smaller_request() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.heic");
    fs::write(&path, b"heif").unwrap();
    let key = preview_cache_key(&path, HEIF_CACHE_VERSION, 4_096).unwrap();
    let cached = directory.path().join(format!("{key}.decoded.jpg"));
    write_jpeg_atomically(
        &DynamicImage::new_rgb8(32, 16),
        &cached,
        90,
        HEIF_DECODED_JPEG,
    )
    .unwrap();

    let result = larger_cached_preview(&path, directory.path(), HEIF_CACHE_VERSION, 512)
        .unwrap()
        .unwrap();

    assert_eq!(result.path, cached);
    assert_eq!((result.width, result.height), (32, 16));
}

#[test]
fn larger_cached_preview_serves_any_backend_tag() {
    // The unified cache lookup must work for non-HEIF backends too. A
    // synthetic RAW-style tag with a 4096 entry should satisfy a 512
    // request without re-decoding.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.arw");
    fs::write(&path, b"raw").unwrap();
    let raw_tag = "libraw-0.22.2-v6";
    let key = preview_cache_key(&path, raw_tag, 4_096).unwrap();
    let cached = directory.path().join(format!("{key}.decoded.jpg"));
    write_jpeg_atomically(
        &DynamicImage::new_rgb8(48, 24),
        &cached,
        90,
        RAW_DEVELOPED_JPEG,
    )
    .unwrap();

    let result = larger_cached_preview(&path, directory.path(), raw_tag, 512)
        .unwrap()
        .unwrap();

    assert_eq!(result.path, cached);
}

#[test]
fn heif_preview_reports_cold_backend_timing_breakdown() {
    let Some(path) = crate::sony_hif_fixture() else {
        eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
        return;
    };
    let cache = tempfile::tempdir().unwrap();

    let preview = heif_preview(&path, cache.path(), 512).unwrap();
    let diagnostics = preview.diagnostics.expect("cold preview diagnostics");

    assert!(diagnostics.backend.is_some());
    assert!(diagnostics.queue_wait_ms.is_some());
    assert!(diagnostics.source_wait_ms.is_some());
    assert!(diagnostics.decode_ms.is_some());
    assert!(diagnostics.encode_ms.is_some());
    assert!(diagnostics.cache_sync_ms.is_some());
    assert!(diagnostics.cache_commit_ms.is_some());
    assert!(diagnostics.total_ms.is_some());
    assert!(
        diagnostics.total_ms.unwrap()
            >= diagnostics.decode_ms.unwrap()
                + diagnostics.encode_ms.unwrap()
                + diagnostics.cache_sync_ms.unwrap()
                + diagnostics.cache_commit_ms.unwrap()
    );

    let warm = heif_preview(&path, cache.path(), 512).unwrap();
    assert!(
        warm.diagnostics.is_none(),
        "warm cache hits skip decode timing"
    );
}

#[test]
fn reads_regular_image_dimensions_without_libraw() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.png");
    RgbImage::from_pixel(7, 5, Rgb([10, 20, 30]))
        .save(&path)
        .unwrap();

    assert_eq!(
        dimensions(&path).unwrap(),
        ImageDimensions {
            width: 7,
            height: 5
        }
    );
}

#[test]
fn near_full_raw_preview_accepts_orientation_and_active_area_difference() {
    assert!(covers_raw_source(
        ImageDimensions {
            width: 7_008,
            height: 4_672,
        },
        ImageDimensions {
            width: 4_688,
            height: 7_028,
        },
    ));
    assert!(!covers_raw_source(
        ImageDimensions {
            width: 1_616,
            height: 1_080,
        },
        ImageDimensions {
            width: 6_240,
            height: 4_168,
        },
    ));
}

#[test]
fn raw_preview_reports_embedded_and_development_failures() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("broken.arw");
    fs::write(&path, b"not a raw image").unwrap();

    let error = raw_preview(&path, directory.path(), 4_096)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("LibRaw failed"));
    #[cfg(target_os = "macos")]
    {
        assert!(error.contains("AppleCoreImage"));
        assert!(error.contains("AppleImageIo"));
    }
    assert!(error.contains("LibRawDevelopment"));
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "writes comparison JPEGs to OXY_RAW_COMPARISON_DIR"]
fn writes_raw_backend_comparison_artifacts() {
    use std::fmt::Write as _;

    let source =
        fs::canonicalize(workspace_path(std::env::var_os("OXY_RAW_FIXTURE").unwrap())).unwrap();
    let output = PathBuf::from(std::env::var_os("OXY_RAW_COMPARISON_DIR").unwrap());
    fs::create_dir_all(&output).unwrap();
    let mut manifest = String::from("backend\tlevel\twidth\theight\tbytes\telapsed_ms\n");

    for (label, max_size) in [
        ("thumbnail-512", Some(512)),
        ("preview-4096", Some(4_096)),
        ("full", None),
    ] {
        for backend in [
            PlannedRawBackend::AppleCoreImage,
            PlannedRawBackend::AppleImageIo,
            PlannedRawBackend::LibRawDevelopment,
        ] {
            let backend_name = match backend {
                PlannedRawBackend::AppleCoreImage => "core-image",
                PlannedRawBackend::AppleImageIo => "image-io",
                PlannedRawBackend::LibRawDevelopment => "libraw",
            };
            let destination = output.join(format!("{backend_name}-{label}.jpg"));
            let started = Instant::now();
            match backend {
                PlannedRawBackend::AppleCoreImage => {
                    apple_core_image::render_raw_jpeg(&source, &destination, max_size, 90).unwrap();
                }
                PlannedRawBackend::AppleImageIo => {
                    apple_image_io::render_jpeg(&source, &destination, max_size, 90).unwrap();
                }
                PlannedRawBackend::LibRawDevelopment => {
                    let image = libraw::developed(&source, max_size).unwrap();
                    write_jpeg_atomically(&image, &destination, 90, RAW_DEVELOPED_JPEG).unwrap();
                }
            }
            let result = preview_result(destination.clone(), PreviewKind::Developed).unwrap();
            writeln!(
                manifest,
                "{backend_name}\t{label}\t{}\t{}\t{}\t{}",
                result.width,
                result.height,
                fs::metadata(destination).unwrap().len(),
                started.elapsed().as_millis(),
            )
            .unwrap();
        }
    }
    fs::write(output.join("manifest.tsv"), manifest).unwrap();
}

#[test]
#[ignore = "requires OXY_RAW_FIXTURE to point to a camera RAW file"]
fn extracts_preview_from_raw_fixture() {
    let raw_path =
        fs::canonicalize(workspace_path(std::env::var_os("OXY_RAW_FIXTURE").unwrap())).unwrap();
    let directory = tempfile::tempdir().unwrap();

    let raw_size = raw::dimensions(&raw_path).unwrap();
    let preview = raw_preview(&raw_path, directory.path(), 512).unwrap();
    let preview_size = dimensions(&preview.path).unwrap();

    assert!(raw_size.width > 0 && raw_size.height > 0);
    assert!(preview_size.width <= 512 && preview_size.height <= 512);
    assert_eq!(
        raw_size.width >= raw_size.height,
        preview_size.width >= preview_size.height,
        "preview orientation must match RAW output dimensions"
    );
    assert!(preview.path.is_file());
}

#[test]
#[ignore = "requires OXY_RAW_FIXTURE to point to a camera RAW file with an embedded JPEG"]
fn preserves_embedded_jpeg_for_loupe_fixture() {
    let raw_path =
        fs::canonicalize(workspace_path(std::env::var_os("OXY_RAW_FIXTURE").unwrap())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let embedded = match libraw::embedded(&raw_path, 4_096).unwrap() {
        libraw::Preview::EmbeddedJpeg(data) => data,
        libraw::Preview::EmbeddedImage(_) => {
            panic!("fixture did not expose an embedded JPEG")
        }
    };

    let preview = raw_preview(&raw_path, directory.path(), 4_096).unwrap();
    assert_eq!(preview.kind, PreviewKind::Embedded);
    assert_eq!(fs::read(&preview.path).unwrap(), embedded);

    let raw_size = raw::dimensions(&raw_path).unwrap();
    let reader = ImageReader::with_format(Cursor::new(&embedded), ImageFormat::Jpeg);
    let mut decoder = reader.into_decoder().unwrap();
    let orientation = decoder.orientation().unwrap();
    let mut oriented_preview = DynamicImage::from_decoder(decoder).unwrap();
    oriented_preview.apply_orientation(orientation);
    let preview_size = ImageDimensions {
        width: oriented_preview.width(),
        height: oriented_preview.height(),
    };
    assert_eq!(
        raw_size.width >= raw_size.height,
        preview_size.width >= preview_size.height,
        "direct embedded preview orientation must match RAW output dimensions"
    );

    assert!(
        matches!(
            libraw::embedded(&raw_path, 512).unwrap(),
            libraw::Preview::EmbeddedJpeg(_)
        ),
        "thumbnail requests must preserve the selected embedded JPEG"
    );
}

#[test]
#[ignore = "requires OXY_RAW_FIXTURE to point to a camera RAW file"]
fn resolves_full_detail_raw_fixture() {
    let raw_path =
        fs::canonicalize(workspace_path(std::env::var_os("OXY_RAW_FIXTURE").unwrap())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let raw_size = raw::dimensions(&raw_path).unwrap();
    let started = Instant::now();
    let full = raw::full(&raw_path, directory.path()).unwrap();

    eprintln!("{}: full RAW={:?}", raw_path.display(), started.elapsed());
    assert!(
        full.kind == PreviewKind::Developed
            || (full.kind == PreviewKind::Embedded
                && covers_raw_source(
                    ImageDimensions {
                        width: full.width,
                        height: full.height,
                    },
                    raw_size,
                ))
    );
    assert!(full.path.is_file());
    assert_eq!(raw::full(&raw_path, directory.path()).unwrap(), full);
}

#[test]
#[ignore = "requires OXY_HEIF_FIXTURE to point to a camera HEIF file"]
fn decodes_full_resolution_heif_fixture() {
    let heif_path = fs::canonicalize(workspace_path(
        std::env::var_os("OXY_HEIF_FIXTURE").unwrap(),
    ))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let original_size = libheif::dimensions(&heif_path).unwrap();
    let started = Instant::now();
    let full = heif_full(&heif_path, directory.path()).unwrap();

    eprintln!("{}: full HEIF={:?}", heif_path.display(), started.elapsed());
    assert_eq!(full.kind, PreviewKind::Decoded);
    assert_eq!(
        (full.width, full.height),
        (original_size.width, original_size.height)
    );
    assert!(full.path.is_file());
    assert_eq!(heif_full(&heif_path, directory.path()).unwrap(), full);

    let mut decoder = ImageReader::open(&full.path)
        .unwrap()
        .into_decoder()
        .unwrap();
    assert!(decoder.icc_profile().unwrap().is_some());
}

#[test]
#[ignore = "requires OXY_HEIF_FIXTURE and macOS ImageIO"]
fn generates_large_image_io_fallback_for_heif_fixture() {
    let heif_path = fs::canonicalize(workspace_path(
        std::env::var_os("OXY_HEIF_FIXTURE").unwrap(),
    ))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let original = libheif::dimensions(&heif_path).unwrap();
    let preview = system_preview(
        &heif_path,
        directory.path(),
        original.width.max(original.height).min(8_192),
    )
    .unwrap();

    assert_eq!(preview.kind, PreviewKind::System);
    assert_eq!(
        (preview.width, preview.height),
        (original.width, original.height)
    );
}

#[test]
#[ignore = "requires local ARW/HIF fixtures; run explicitly in release mode"]
fn fixture_preview_performance_budgets() {
    let fixture_dir = std::env::var_os("OXY_MEDIA_FIXTURE_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/media"),
        workspace_path,
    );
    let mut fixtures = fs::read_dir(&fixture_dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", fixture_dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("arw") | Some("hif")
            )
        })
        .collect::<Vec<_>>();
    fixtures.sort();
    assert!(!fixtures.is_empty(), "no ARW/HIF fixtures found");

    for fixture in fixtures {
        let cache = tempfile::tempdir().unwrap();
        let preview = |size| {
            if fixture
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("arw"))
            {
                raw_preview(&fixture, cache.path(), size)
            } else {
                system_preview(&fixture, cache.path(), size)
            }
        };

        let thumbnail_started = Instant::now();
        preview(512).unwrap();
        let thumbnail_elapsed = thumbnail_started.elapsed();

        let loupe_started = Instant::now();
        let loupe_path = preview(4_096).unwrap();
        let loupe_elapsed = loupe_started.elapsed();

        let warm_started = Instant::now();
        assert_eq!(preview(4_096).unwrap(), loupe_path);
        let warm_elapsed = warm_started.elapsed();

        eprintln!(
            "{}: thumbnail={thumbnail_elapsed:?} loupe={loupe_elapsed:?} warm={warm_elapsed:?}",
            fixture.display()
        );
        assert!(
            thumbnail_elapsed < Duration::from_millis(800),
            "{} thumbnail took {thumbnail_elapsed:?}",
            fixture.display()
        );
        assert!(
            loupe_elapsed < Duration::from_millis(800),
            "{} loupe took {loupe_elapsed:?}",
            fixture.display()
        );
        assert!(
            warm_elapsed < Duration::from_millis(150),
            "{} warm cache took {warm_elapsed:?}",
            fixture.display()
        );
    }
}

#[test]
#[ignore = "requires OXY_HEIF_FIXTURE to point to a HEIF file"]
fn heif_decode_performance_budget() {
    let heif_path = fs::canonicalize(workspace_path(
        std::env::var_os("OXY_HEIF_FIXTURE").unwrap(),
    ))
    .unwrap();

    // Cold full decode + PNG cache write.
    let cache_full = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let full = heif_full(&heif_path, cache_full.path()).unwrap();
    let full_elapsed = started.elapsed();
    eprintln!("HEIF full decode: {full_elapsed:?}");
    assert!(
        full_elapsed < Duration::from_secs(3),
        "full decode took {full_elapsed:?}"
    );

    // Warm full-detail cache hit.
    let started = Instant::now();
    assert_eq!(
        heif_full(&heif_path, cache_full.path()).unwrap().path,
        full.path
    );
    let warm_full = started.elapsed();
    eprintln!("HEIF warm full: {warm_full:?}");
    assert!(
        warm_full < Duration::from_millis(100),
        "warm full took {warm_full:?}"
    );

    // Cold 512 px thumbnail via decode_scaled (8-bit fast path).
    let cache_thumb = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let thumb = heif_preview(&heif_path, cache_thumb.path(), 512).unwrap();
    let thumb_elapsed = started.elapsed();
    eprintln!("HEIF thumbnail (512): {thumb_elapsed:?}");
    assert!(
        thumb_elapsed < Duration::from_millis(800),
        "thumbnail took {thumb_elapsed:?}"
    );
    assert!(thumb.width <= 512 && thumb.height <= 512);

    // Cold 4096 px loupe preview via decode_scaled.
    let cache_loupe = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let loupe = heif_preview(&heif_path, cache_loupe.path(), 4_096).unwrap();
    let loupe_elapsed = started.elapsed();
    eprintln!("HEIF loupe (4096): {loupe_elapsed:?}");
    assert!(
        loupe_elapsed < Duration::from_millis(1_500),
        "loupe took {loupe_elapsed:?}"
    );
    assert!(loupe.width <= 4_096 && loupe.height <= 4_096);

    // Warm loupe cache hit.
    let started = Instant::now();
    assert_eq!(
        heif_preview(&heif_path, cache_loupe.path(), 4_096)
            .unwrap()
            .path,
        loupe.path
    );
    let warm_loupe = started.elapsed();
    eprintln!("HEIF warm loupe: {warm_loupe:?}");
    assert!(
        warm_loupe < Duration::from_millis(100),
        "warm loupe took {warm_loupe:?}"
    );
}
