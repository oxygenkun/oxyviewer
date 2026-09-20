use oxy_analyzer_host::{
    AnalyzeInput, Analyzer, BuiltInFaceAnalyzer, ProcessAnalyzer, verify_models, verify_pack,
};
use oxy_domain::{AnalyzerPackManifest, FaceAnalyzerSettings};
use oxy_runtime::CancellationToken;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn manifest(root: &Path, binary: &Path) -> AnalyzerPackManifest {
    let name = binary.file_name().unwrap().to_str().unwrap();
    fs::copy(binary, root.join(name)).unwrap();
    let manifest = AnalyzerPackManifest {
        id: "faces".into(),
        version: "1".into(),
        host_api: 2,
        target_os: std::env::consts::OS.into(),
        target_arch: std::env::consts::ARCH.into(),
        entrypoint: name.into(),
        entrypoint_sha256: format!("{:x}", Sha256::digest(fs::read(binary).unwrap())),
    };
    write_manifest(root, &manifest);
    manifest
}
fn write_manifest(root: &Path, manifest: &AnalyzerPackManifest) {
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
}
fn models() -> Option<PathBuf> {
    let root = std::env::var_os("OXY_FACE_MODEL_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/native/face-models"),
        PathBuf::from,
    );
    if !root.is_dir() {
        eprintln!("skipping model test: set OXY_FACE_MODEL_DIR to the managed models");
        return None;
    }
    Some(root)
}
#[test]
fn rejects_modified_binary_wrong_api_and_path_escape() {
    let root = tempfile::tempdir().unwrap();
    let mut pack = manifest(
        root.path(),
        Path::new(env!("CARGO_BIN_EXE_oxy-face-worker")),
    );
    assert!(verify_pack(root.path()).is_ok());
    pack.host_api = 99;
    write_manifest(root.path(), &pack);
    assert!(verify_pack(root.path()).is_err());
    pack.host_api = 2;
    pack.entrypoint = "../escape".into();
    write_manifest(root.path(), &pack);
    assert!(verify_pack(root.path()).is_err());
    pack.entrypoint = "oxy-face-worker".into();
    pack.entrypoint_sha256 = "bad".into();
    write_manifest(root.path(), &pack);
    assert!(verify_pack(root.path()).is_err());
}
#[test]
fn real_subprocess_matches_builtin_for_portrait_and_empty_image() {
    let Some(models) = models() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    manifest(
        root.path(),
        Path::new(env!("CARGO_BIN_EXE_oxy-face-worker")),
    );
    let settings = FaceAnalyzerSettings::default();
    let cancel = CancellationToken::default();
    let process = ProcessAnalyzer::load(root.path(), &models, settings, &cancel).unwrap();
    let builtin = BuiltInFaceAnalyzer::load(&verify_models(&models).unwrap(), settings).unwrap();
    assert_eq!(process.descriptor(), builtin.descriptor());
    let photo = image::open(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../oxy-faces/tests/data/fixtures/two_people.jpg"),
    )
    .unwrap()
    .into_rgb8();
    for image in [photo, image::RgbImage::new(64, 64)] {
        let input = AnalyzeInput {
            asset_id: "asset",
            source_revision: "revision",
            width: image.width(),
            height: image.height(),
            pixels: image.as_raw(),
        };
        let actual = process.analyze(&input, &cancel).unwrap();
        let expected = builtin.analyze(&input, &cancel).unwrap();
        assert_eq!(actual.regions, expected.regions);
        assert_eq!(actual.features, expected.features);
    }
    let large = vec![127; 2048 * 2048 * 3];
    let start = std::time::Instant::now();
    std::thread::scope(|scope| {
        let token = cancel.clone();
        scope.spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(25));
            token.cancel();
        });
        let result = process.analyze(
            &AnalyzeInput {
                asset_id: "large",
                source_revision: "r",
                width: 2048,
                height: 2048,
                pixels: &large,
            },
            &cancel,
        );
        assert!(matches!(
            result,
            Err(oxy_analyzer_host::AnalyzerError::Cancelled)
        ));
    });
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    cancel.cancel();
    assert!(matches!(
        process.analyze(
            &AnalyzeInput {
                asset_id: "a",
                source_revision: "r",
                width: 1,
                height: 1,
                pixels: &[0, 0, 0]
            },
            &cancel
        ),
        Err(oxy_analyzer_host::AnalyzerError::Cancelled)
    ));
}
