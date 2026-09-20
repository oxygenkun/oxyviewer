//! Product-model integration and deterministic geometry checks.

use std::path::{Path, PathBuf};

use oxy_domain::FaceAnalyzerSettings;
use oxy_faces::{
    FaceAnalyzer, FaceModelPaths, RgbImage, ScrfdDetector, align_face, letterbox,
    managed_model_paths,
};
use serde::Deserialize;

fn settings() -> FaceAnalyzerSettings {
    FaceAnalyzerSettings {
        detection_confidence: 0.5,
        nms_threshold: 0.3,
        max_faces_per_asset: 5000,
        min_face_pixels: 8,
        detect_small_faces: false,
        ..FaceAnalyzerSettings::default()
    }
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn data_dir() -> PathBuf {
    manifest_dir().join("tests/data")
}

fn model_paths() -> Option<FaceModelPaths> {
    let directory = std::env::var_os("OXY_FACE_MODEL_DIR").map_or_else(
        || manifest_dir().join("../../target/native/face-models"),
        PathBuf::from,
    );
    managed_model_paths(&directory)
}

fn load_image(path: &Path) -> RgbImage {
    let image = image::open(path)
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()))
        .to_rgb8();
    RgbImage::new(image.width(), image.height(), image.into_raw()).expect("decoded RGB8")
}

fn synthetic_rgb(width: u32, height: u32) -> RgbImage {
    let mut data = Vec::with_capacity(width as usize * height as usize * 3);
    for y in 0..height as i64 {
        for x in 0..width as i64 {
            data.push(((x * 3 + y * 5) % 256) as u8);
            data.push(((x * 7 + y * 11) % 256) as u8);
            data.push(((x * 13 + y * 17) % 256) as u8);
        }
    }
    RgbImage::new(width, height, data).expect("synthetic RGB8")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlignGolden {
    landmarks: Vec<[f32; 2]>,
    aligned_sum: i64,
}

#[test]
fn alignment_matches_opencv_warp_affine() {
    let golden: AlignGolden = serde_json::from_str(
        &std::fs::read_to_string(data_dir().join("golden/align_synthetic.json")).unwrap(),
    )
    .unwrap();
    let canvas = synthetic_rgb(320, 320);
    let aligned = align_face(&canvas, &golden.landmarks, 112).expect("alignment succeeds");
    let reference = load_image(&data_dir().join("golden/align_synthetic_aligned.png"));
    let total = aligned
        .data()
        .iter()
        .map(|value| i64::from(*value))
        .sum::<i64>();
    let difference = aligned
        .data()
        .iter()
        .zip(reference.data())
        .map(|(left, right)| (i64::from(*left) - i64::from(*right)).abs())
        .sum::<i64>();
    let brightness_budget = (golden.aligned_sum.abs() as f64 * 1e-4).max(512.0);
    assert!((total - golden.aligned_sum).abs() as f64 <= brightness_budget);
    assert!(difference as f64 / (aligned.data().len() as f64) < 3.0);
}

#[test]
#[ignore = "requires the user-downloadable SCRFD and AdaFace models"]
fn managed_models_run_through_the_full_pipeline() {
    let paths = model_paths().expect("download SCRFD and AdaFace first");
    let analyzer = FaceAnalyzer::load(&paths, settings()).expect("managed models load");
    let input = load_image(&data_dir().join("fixtures/two_people.jpg"));
    let faces = analyzer
        .analyze("asset", Path::new("/tmp/managed.jpg"), "rev-1", &input)
        .expect("analysis succeeds");
    assert_eq!(faces.len(), 2);
    assert_eq!(analyzer.embedding_dim(), 512);
    assert!(faces.iter().all(|face| face.embedding.len() == 512));
    assert!(faces.iter().all(|face| {
        let norm = face
            .embedding
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        (norm - 1.0).abs() < 1e-4
    }));
}

#[test]
#[ignore = "requires the user-downloadable SCRFD and AdaFace models"]
fn analysis_is_idempotent_and_revision_sensitive() {
    let paths = model_paths().expect("download SCRFD and AdaFace first");
    let analyzer = FaceAnalyzer::load(&paths, settings()).expect("managed models load");
    let input = load_image(&data_dir().join("fixtures/two_people.jpg"));
    let first = analyzer
        .analyze("asset", Path::new("/tmp/a.jpg"), "rev-1", &input)
        .unwrap();
    let second = analyzer
        .analyze("asset", Path::new("/tmp/a.jpg"), "rev-1", &input)
        .unwrap();
    let third = analyzer
        .analyze("asset", Path::new("/tmp/a.jpg"), "rev-2", &input)
        .unwrap();
    assert_eq!(first, second);
    assert_ne!(
        first[0].observation.observation_id,
        third[0].observation.observation_id
    );
}

#[test]
#[ignore = "local release-mode SCRFD timing diagnostic"]
fn profile_scrfd_10g_kps() {
    let paths = model_paths().expect("download SCRFD first");
    let image = load_image(&data_dir().join("fixtures/two_people.jpg"));
    for size in [640, 512, 480] {
        let boxed = letterbox(&image, size);
        let started = std::time::Instant::now();
        let detector = ScrfdDetector::load(&paths.detector, size).expect("SCRFD loads");
        let load = started.elapsed();
        let started = std::time::Instant::now();
        let faces = detector.detect(&boxed, &settings()).expect("SCRFD detects");
        let first = started.elapsed();
        let started = std::time::Instant::now();
        for _ in 0..3 {
            detector.detect(&boxed, &settings()).expect("SCRFD detects");
        }
        eprintln!(
            "SCRFD-10G {size} CPU: load={load:?}, first={first:?}, warm-average={:?}, faces={}",
            started.elapsed() / 3,
            faces.len()
        );
    }
}
