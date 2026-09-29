//! Opt-in, equal-input backend benchmark; does not alter application settings.
use super::*;
use crate::execution::operations::PersonOperations;
use image::{RgbImage, imageops::FilterType};
use oxy_domain::{PersonOperationState, PersonOperationStatus};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Instant};

fn milliseconds(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn distribution(values: &[f64]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!({"n": sorted.len(), "mean_ms": sorted.iter().sum::<f64>() / sorted.len() as f64,
        "p50_ms": sorted[sorted.len()/2],
        "p95_ms": sorted[(sorted.len() as f64 * 0.95).ceil() as usize - 1]})
}

fn load(root: &Path, runtime: &Path, provider: OnnxProvider) -> Result<OnnxFaceModels, String> {
    let manifest = crate::environment::catalog::manifest();
    OnnxFaceModels::load_verified(OnnxFaceModelRequest {
        manifest: &manifest,
        detector_stage_id: crate::environment::catalog::DETECTOR,
        encoder_stage_id: crate::environment::catalog::ENCODER,
        store_dir: root,
        runtime_library: runtime,
        runtime_sha256: runtime_install::RUNTIME_SHA,
        directml_sha256: Some(runtime_install::DML_SHA),
        detector_canvas: 960,
        providers: OnnxFaceProviders {
            detector: provider,
            encoder: provider,
        },
    })
    .map_err(|e| e.to_string())
}

type Reference = Vec<(Vec<FaceDetection>, Vec<Vec<f32>>)>;

fn measure(
    root: &Path,
    runtime: &Path,
    provider: OnnxProvider,
    images: &[RgbImage],
    reference: &mut Reference,
) -> Result<Value, String> {
    let started = Instant::now();
    let mut models = load(root, runtime, provider)?;
    let load_ms = milliseconds(started);
    eprintln!("BENCH loaded in {load_ms:.1} ms");
    let started = Instant::now();
    let faces = models
        .detect_faces(&images[0], 0.5, 0.4)
        .map_err(|e| e.to_string())?;
    for face in &faces {
        models
            .encode_face(&images[0], face.landmarks)
            .map_err(|e| e.to_string())?;
    }
    let first_image_ms = milliseconds(started);
    eprintln!("BENCH first image {first_image_ms:.1} ms");
    let is_reference = reference.is_empty();
    let mut detections = Vec::new();
    let mut encodings = Vec::new();
    let mut totals = Vec::new();
    let mut rounds = Vec::new();
    let mut counts = Vec::new();
    let mut minimum_cosine = 1.0_f32;
    let mut image_cosines = vec![1.0_f32; images.len()];
    let mut count_mismatches = 0;
    for round in 0..3 {
        let mut batch_ms = 0.0;
        for index in 0..images.len() {
            let started = Instant::now();
            let faces = models
                .detect_faces(&images[index], 0.5, 0.4)
                .map_err(|e| e.to_string())?;
            let detect_ms = milliseconds(started);
            detections.push(detect_ms);
            let expected_faces = if is_reference && round == 0 {
                &faces
            } else {
                &reference[index].0
            };
            if faces.len() != expected_faces.len() {
                count_mismatches += 1;
            }
            let mut feature_ms = 0.0;
            let mut features = Vec::new();
            for (face_index, face) in expected_faces.iter().enumerate() {
                let started = Instant::now();
                let feature = models
                    .encode_face(&images[index], face.landmarks)
                    .map_err(|e| e.to_string())?;
                let encode_ms = milliseconds(started);
                encodings.push(encode_ms);
                feature_ms += encode_ms;
                if !is_reference {
                    let cosine: f32 = feature
                        .iter()
                        .zip(&reference[index].1[face_index])
                        .map(|(a, b)| a * b)
                        .sum();
                    minimum_cosine = minimum_cosine.min(cosine);
                    image_cosines[index] = image_cosines[index].min(cosine);
                }
                features.push(feature);
            }
            if round == 0 {
                counts.push(faces.len());
            }
            if is_reference && round == 0 {
                reference.push((faces, features));
            }
            totals.push(detect_ms + feature_ms);
            batch_ms += detect_ms + feature_ms;
        }
        rounds.push(batch_ms);
    }
    Ok(json!({"load_ms":load_ms, "first_image_ms":first_image_ms,
        "detection":distribution(&detections), "encoding_per_face":distribution(&encodings),
        "total_per_image":distribution(&totals), "round_totals_ms":rounds,
        "face_counts":counts, "face_count_mismatches":count_mismatches,
        "minimum_cpu_embedding_cosine": minimum_cosine,
        "per_image_minimum_cosines": image_cosines,
        "output_check_passed": count_mismatches == 0 && minimum_cosine > 0.99}))
}

#[test]
#[ignore = "manual release benchmark: installed WinML EPs/models and a real JPEG folder"]
fn compare_installed_backends() {
    let root = PathBuf::from(std::env::var_os("OXY_TEST_MODEL_ROOT").unwrap());
    let folder = PathBuf::from(std::env::var_os("OXY_BENCH_IMAGE_DIR").unwrap());
    let output = PathBuf::from(std::env::var_os("OXY_BENCH_OUTPUT").unwrap());
    let mut paths: Vec<_> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("jpg"))
        })
        .collect();
    paths.sort();
    assert!(paths.len() >= 16);
    let mut inputs = Vec::new();
    let mut images = Vec::new();
    for index in 0..16 {
        let path = &paths[index * (paths.len() - 1) / 15];
        let bytes = std::fs::read(path).unwrap();
        let started = Instant::now();
        let original = image::load_from_memory(&bytes).unwrap().to_rgb8();
        let decoded_ms = milliseconds(started);
        let (width, height) = original.dimensions();
        let edge = width.max(height);
        let started = Instant::now();
        let image = if edge > 1600 {
            image::imageops::resize(
                &original,
                (u64::from(width) * 1600 / u64::from(edge)) as u32,
                (u64::from(height) * 1600 / u64::from(edge)) as u32,
                FilterType::Triangle,
            )
        } else {
            original
        };
        inputs.push(json!({"file":path.file_name().unwrap().to_string_lossy(),
            "sha256":format!("{:x}", Sha256::digest(&bytes)), "source_size":[width,height],
            "input_size":[image.width(),image.height()], "decode_ms":decoded_ms,"resize_ms":milliseconds(started)}));
        images.push(image);
    }
    let runtime = runtime_install::installed(&root).unwrap().unwrap();
    initialize_runtime(&runtime).unwrap();
    let operations = PersonOperations::default();
    let work = operations
        .reserve(PersonOperationStatus {
            operation_id: "benchmark".into(),
            folder_path: None,
            state: PersonOperationState::Preparing,
            completed: 0,
            total: 0,
            detail: String::new(),
            error: None,
            run: None,
        })
        .unwrap();
    let started = Instant::now();
    let warnings = install_and_register(&runtime, &work).unwrap();
    let catalog_ms = milliseconds(started);
    let env = Environment::current().unwrap();
    let devices: Vec<_> = env
        .devices()
        .enumerate()
        .map(|(index, d)| {
            json!({"index":index,
        "ep":d.ep().unwrap(),"type":format!("{:?}",d.hardware_device().ty()),
        "vendor":d.hardware_device().vendor().unwrap_or("unknown"),"id":d.hardware_device().id()})
        })
        .collect();
    let mut candidates = vec![(
        "CPU (2 threads)".to_string(),
        OnnxProvider::Cpu { intra_threads: 2 },
    )];
    let mut seen = HashSet::new();
    let selected_eps = std::env::var("OXY_BENCH_EPS").ok();
    let mut hardware: Vec<_> = env.devices().enumerate().collect();
    hardware.sort_by_key(|(_, d)| {
        device_priority(d.ep().unwrap(), d.hardware_device().ty()).unwrap_or(u8::MAX)
    });
    for (index, d) in hardware {
        let ep = d.ep().unwrap();
        if selected_eps
            .as_ref()
            .is_some_and(|selected| !selected.split(',').any(|name| name == ep))
        {
            continue;
        }
        if !matches!(
            ep,
            "DmlExecutionProvider" | "OpenVINOExecutionProvider" | "NvTensorRTRTXExecutionProvider"
        ) || !seen.insert((
            ep.to_string(),
            format!("{:?}", d.hardware_device().ty()),
            d.hardware_device().id(),
        )) {
            continue;
        }
        candidates.push((
            format!(
                "{} / {:?} / {}",
                d.ep().unwrap(),
                d.hardware_device().ty(),
                d.hardware_device().id()
            ),
            OnnxProvider::WindowsDevice {
                index,
                intra_threads: 2,
            },
        ));
    }
    let mut results = Vec::new();
    let mut reference = Vec::new();
    for (name, provider) in candidates {
        eprintln!("BENCH starting {name}");
        let result = match measure(&root, &runtime, provider, &images, &mut reference) {
            Ok(metrics) => json!({"name":name,"metrics":metrics}),
            Err(error) => json!({"name":name,"error":error}),
        };
        eprintln!("BENCH {result}");
        results.push(result);
        std::fs::write(
            &output,
            serde_json::to_vec_pretty(&json!({"inputs":inputs,"devices":devices,
            "catalog_ms":catalog_ms,"catalog_warnings":warnings,"rounds":3,"results":results}))
            .unwrap(),
        )
        .unwrap();
    }
    assert_eq!(reference.len(), images.len());
}
