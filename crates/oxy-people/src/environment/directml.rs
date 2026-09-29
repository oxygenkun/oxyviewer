//! Desktop Windows policy: standalone ORT, one explicit DirectML GPU, no WinML.
use super::{catalog, ort};
use crate::{execution::operations::PersonOperation, inference::face::*};
use std::path::Path;

pub fn load_models(root: &Path, operation: &PersonOperation) -> Result<OnnxFaceModels, String> {
    if operation.is_cancelled() {
        return Err("已取消".into());
    }
    let runtime = ort::installed(root)?.ok_or("请先下载 ORT DirectML 运行时")?;
    let manifest = catalog::manifest();
    let provider = OnnxProvider::DirectMl {
        device_id: 0,
        intra_threads: 2,
    };
    operation.update(|s| s.detail = "正在加载 ORT DirectML GPU 模型…".into());
    let mut models = OnnxFaceModels::load_verified(OnnxFaceModelRequest {
        manifest: &manifest,
        detector_stage_id: catalog::DETECTOR,
        encoder_stage_id: catalog::ENCODER,
        store_dir: root,
        runtime_library: &runtime,
        runtime_sha256: ort::RUNTIME_SHA,
        directml_sha256: Some(ort::DML_SHA),
        detector_canvas: 960,
        providers: OnnxFaceProviders {
            detector: provider,
            encoder: provider,
        },
    })
    .map_err(|e| e.to_string())?;
    if operation.is_cancelled() {
        return Err("已取消".into());
    }
    operation.update(|s| s.detail = "正在验证 ORT DirectML GPU 推理…".into());
    let pixels = image::RgbImage::from_pixel(112, 112, image::Rgb([127, 127, 127]));
    models.detect_raw(&pixels).map_err(|e| e.to_string())?;
    if operation.is_cancelled() {
        return Err("已取消".into());
    }
    models
        .encode_face(
            &pixels,
            [[38., 52.], [74., 52.], [56., 72.], [42., 92.], [71., 92.]],
        )
        .map_err(|e| e.to_string())?;
    if operation.is_cancelled() {
        return Err("已取消".into());
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::operations::PersonOperations;
    use oxy_domain::{PersonOperationState, PersonOperationStatus};

    #[test]
    #[ignore = "installed DML models and paired resize fixture PNGs"]
    fn resized_inputs_preserve_model_outputs() {
        let root = std::path::PathBuf::from(std::env::var_os("OXY_TEST_MODEL_ROOT").unwrap());
        let pairs = std::path::PathBuf::from(std::env::var_os("OXY_RESIZE_PARITY_DIR").unwrap());
        let operations = PersonOperations::default();
        let work = operations
            .reserve(PersonOperationStatus {
                operation_id: "resize-parity".into(),
                folder_path: None,
                state: PersonOperationState::Preparing,
                completed: 0,
                total: 0,
                detail: String::new(),
                error: None,
                run: None,
            })
            .unwrap();
        let mut models = load_models(&root, &work).unwrap();
        let mut rows = Vec::new();
        for i in 0..16 {
            let old = image::open(pairs.join(format!("{i:02}-old.png")))
                .unwrap()
                .to_rgb8();
            let new = image::open(pairs.join(format!("{i:02}-new.png")))
                .unwrap()
                .to_rgb8();
            let mut old_faces = models.detect_faces(&old, 0.5, 0.4).unwrap();
            let mut new_faces = models.detect_faces(&new, 0.5, 0.4).unwrap();
            old_faces.sort_by(|a, b| a.box_xyxy[0].total_cmp(&b.box_xyxy[0]));
            new_faces.sort_by(|a, b| a.box_xyxy[0].total_cmp(&b.box_xyxy[0]));
            assert_eq!(old_faces.len(), new_faces.len());
            for (before, after) in old_faces.iter().zip(&new_faces) {
                let box_error = before
                    .box_xyxy
                    .iter()
                    .zip(after.box_xyxy)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0_f32, f32::max);
                let a = models.encode_face(&old, before.landmarks).unwrap();
                let b = models.encode_face(&new, after.landmarks).unwrap();
                let cosine: f32 = a.iter().zip(b).map(|(a, b)| a * b).sum();
                assert!(
                    box_error < 2. && cosine > 0.995,
                    "photo={i}, box_error={box_error}, cosine={cosine}"
                );
                rows.push(serde_json::json!({"photo":i,"box_max_error_px":box_error,"embedding_cosine":cosine}));
            }
        }
        std::fs::write(
            pairs.join("models.json"),
            serde_json::to_vec_pretty(&rows).unwrap(),
        )
        .unwrap();
    }

    #[test]
    #[ignore = "installs pinned ORT DirectML and verifies real GPU operator execution"]
    fn installed_directml_runs_on_gpu() {
        let root = std::path::PathBuf::from(std::env::var_os("OXY_TEST_MODEL_ROOT").unwrap());
        let image = std::env::var_os("OXY_TEST_FACE_IMAGE").unwrap();
        ort::download(&root, &mut |_, _| true).unwrap();
        let operations = PersonOperations::default();
        let work = operations
            .reserve(PersonOperationStatus {
                operation_id: "ort-test".into(),
                folder_path: None,
                state: PersonOperationState::Preparing,
                completed: 0,
                total: 0,
                detail: String::new(),
                error: None,
                run: None,
            })
            .unwrap();
        let mut models = load_models(&root, &work).unwrap();
        let pixels = image::open(image)
            .unwrap()
            .resize(1600, 1600, image::imageops::FilterType::Triangle)
            .to_rgb8();
        let started = std::time::Instant::now();
        let faces = models.detect_faces(&pixels, 0.5, 0.4).unwrap();
        assert!(!faces.is_empty());
        for face in &faces {
            assert_eq!(
                models.encode_face(&pixels, face.landmarks).unwrap().len(),
                512
            );
        }
        eprintln!(
            "standalone ORT DirectML: {} faces, {:?}",
            faces.len(),
            started.elapsed()
        );
        for path in models.finish_profiles() {
            let events: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let dml = events
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["args"]["provider"] == "DmlExecutionProvider")
                .count();
            eprintln!("profile={path}; DML kernels={dml}");
            assert!(dml > 0, "model did not execute any GPU kernels");
        }
        assert!(
            !root
                .join(ort::VERSION)
                .join("Microsoft.Windows.AI.MachineLearning.dll")
                .exists()
        );
    }
}
