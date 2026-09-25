//! Shared SCRFD/AdaFace inference using a verified platform ONNX Runtime.
//! Sessions belong to a background worker; mutable runs are serialized.
//! Installation and licensing stay in the pipeline manifest and artifact store.

use crate::{
    alignment::detection_cache_id,
    artifact_store::{ArtifactInstallError, installed_artifact},
    face_input::{FaceInputError, adaface_input, scrfd_input},
    require_installable,
};
use image::RgbImage;
use ort::{ep, session::Session, session::builder::GraphOptimizationLevel, value::TensorRef};
use oxy_domain::{
    DetectedPersonInstance, PersonModelArtifactFormat, PersonStageKind, PipelineManifest,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use thiserror::Error;

const EMBEDDING_DIM: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnnxProvider {
    Cpu {
        intra_threads: usize,
    },
    #[cfg(windows)]
    DirectMl {
        device_id: i32,
        intra_threads: usize,
    },
    #[cfg(target_os = "macos")]
    CoreMl {
        compute_units: MacComputeUnits,
        intra_threads: usize,
    },
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacComputeUnits {
    All,
    CpuAndGpu,
    CpuAndNeuralEngine,
}

impl OnnxProvider {
    fn threads(self) -> usize {
        match self {
            Self::Cpu { intra_threads } => intra_threads,
            #[cfg(windows)]
            Self::DirectMl { intra_threads, .. } => intra_threads,
            #[cfg(target_os = "macos")]
            Self::CoreMl { intra_threads, .. } => intra_threads,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnnxFaceProviders {
    pub detector: OnnxProvider,
    pub encoder: OnnxProvider,
}

#[derive(Debug, Error)]
pub enum OnnxFaceError {
    #[error(transparent)]
    Artifact(#[from] ArtifactInstallError),
    #[error(transparent)]
    FaceInput(#[from] FaceInputError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("ONNX Runtime or companion library does not match its expected SHA-256")]
    RuntimeDigestMismatch,
    #[error("a different ONNX Runtime library is already loaded in this process")]
    RuntimeAlreadyLoaded,
    #[error("invalid model, runtime, provider, or tensor contract")]
    InvalidContract,
    #[error("required model artifact is not installed")]
    ModelNotInstalled,
    #[error("ONNX Runtime failed: {0}")]
    Ort(String),
}

#[derive(Debug)]
pub struct OnnxTensor {
    pub name: String,
    pub shape: Vec<i64>,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, Copy)]
pub struct ScrfdGeometry {
    pub scale: f32,
    pub resized_width: u32,
    pub resized_height: u32,
    pub canvas_size: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FaceDetection {
    /// Source-image pixel coordinates, which may extend past the image edge.
    pub box_xyxy: [f32; 4],
    pub landmarks: [[f32; 2]; 5],
    pub score: f32,
}

pub struct OnnxFaceModelRequest<'a> {
    pub manifest: &'a PipelineManifest,
    pub detector_stage_id: &'a str,
    pub encoder_stage_id: &'a str,
    pub store_dir: &'a Path,
    pub runtime_library: &'a Path,
    pub runtime_sha256: &'a str,
    #[cfg(windows)]
    pub directml_sha256: Option<&'a str>,
    #[cfg(target_os = "macos")]
    pub coreml_cache_dir: Option<&'a Path>,
    pub detector_canvas: u32,
    pub providers: OnnxFaceProviders,
}

pub struct OnnxFaceModels {
    detector: Session,
    encoder: Session,
    detector_canvas: u32,
    providers: OnnxFaceProviders,
}

fn digest_matches(path: &Path, expected: &str) -> Result<bool, OnnxFaceError> {
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(OnnxFaceError::InvalidContract);
    }
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()) == expected.to_ascii_lowercase())
}

fn initialize_runtime(path: &Path) -> Result<(), OnnxFaceError> {
    static LOADED_PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    let path = path.canonicalize()?;
    let loaded = LOADED_PATH.get_or_init(|| Mutex::new(None));
    let mut loaded = loaded
        .lock()
        .map_err(|_| OnnxFaceError::RuntimeAlreadyLoaded)?;
    if let Some(existing) = loaded.as_ref() {
        return if existing == &path {
            Ok(())
        } else {
            Err(OnnxFaceError::RuntimeAlreadyLoaded)
        };
    }
    ort::init_from(&path)
        .map_err(|error| OnnxFaceError::Ort(error.to_string()))?
        .commit();
    *loaded = Some(path);
    Ok(())
}

fn build_session(
    path: &Path,
    provider: OnnxProvider,
    batch_dimension: Option<&str>,
    #[cfg(target_os = "macos")] coreml_cache_dir: Option<&Path>,
) -> Result<Session, OnnxFaceError> {
    let mut builder = Session::builder()
        .map_err(|error| OnnxFaceError::Ort(error.to_string()))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|error| OnnxFaceError::Ort(error.to_string()))?
        .with_intra_threads(provider.threads())
        .map_err(|error| OnnxFaceError::Ort(error.to_string()))?;
    if let Some(dimension) = batch_dimension {
        builder = builder
            .with_dimension_override(dimension, 1)
            .map_err(|error| OnnxFaceError::Ort(error.to_string()))?;
    }
    #[cfg(windows)]
    if let OnnxProvider::DirectMl { device_id, .. } = provider {
        builder = builder
            .with_memory_pattern(false)
            .map_err(|error| OnnxFaceError::Ort(error.to_string()))?
            .with_parallel_execution(false)
            .map_err(|error| OnnxFaceError::Ort(error.to_string()))?
            .with_execution_providers([ep::DirectML::default()
                .with_device_id(device_id)
                .build()
                .error_on_failure()])
            .map_err(|error| OnnxFaceError::Ort(error.to_string()))?;
    }
    #[cfg(target_os = "macos")]
    if let OnnxProvider::CoreMl { compute_units, .. } = provider {
        let units = match compute_units {
            MacComputeUnits::All => ep::coreml::ComputeUnits::All,
            MacComputeUnits::CpuAndGpu => ep::coreml::ComputeUnits::CPUAndGPU,
            MacComputeUnits::CpuAndNeuralEngine => ep::coreml::ComputeUnits::CPUAndNeuralEngine,
        };
        let mut coreml = ep::CoreML::default()
            .with_compute_units(units)
            .with_model_format(ep::coreml::ModelFormat::NeuralNetwork)
            .with_static_input_shapes(true);
        if let Some(cache_dir) = coreml_cache_dir {
            coreml = coreml.with_model_cache_dir(cache_dir.to_string_lossy());
        }
        builder = builder
            .with_execution_providers([coreml.build().error_on_failure()])
            .map_err(|error| OnnxFaceError::Ort(error.to_string()))?;
    }
    builder
        .commit_from_file(path)
        .map_err(|error| OnnxFaceError::Ort(error.to_string()))
}

fn model_path(
    manifest: &PipelineManifest,
    stage_id: &str,
    kind: PersonStageKind,
    store_dir: &Path,
) -> Result<PathBuf, OnnxFaceError> {
    let stage = manifest
        .stages
        .iter()
        .find(|stage| stage.stage_id == stage_id && stage.kind == kind)
        .ok_or(OnnxFaceError::InvalidContract)?;
    let artifact = stage
        .artifact
        .as_ref()
        .ok_or(OnnxFaceError::InvalidContract)?;
    if artifact.format != PersonModelArtifactFormat::Onnx {
        return Err(OnnxFaceError::InvalidContract);
    }
    installed_artifact(manifest, &artifact.artifact_id, store_dir)?
        .ok_or(OnnxFaceError::ModelNotInstalled)
}

impl OnnxFaceModels {
    /// Construct only from an installable manifest and hash-verified model
    /// artifacts. The runtime library and any DirectML companion DLL are
    /// separately checked against application-pinned digests.
    pub fn load_verified(request: OnnxFaceModelRequest<'_>) -> Result<Self, OnnxFaceError> {
        let OnnxFaceModelRequest {
            manifest,
            detector_stage_id,
            encoder_stage_id,
            store_dir,
            runtime_library,
            runtime_sha256,
            #[cfg(windows)]
            directml_sha256,
            #[cfg(target_os = "macos")]
            coreml_cache_dir,
            detector_canvas,
            providers,
        } = request;
        require_installable(manifest).map_err(ArtifactInstallError::from)?;
        if !(320..=1280).contains(&detector_canvas)
            || detector_canvas % 32 != 0
            || providers.detector.threads() == 0
            || providers.encoder.threads() == 0
            || {
                #[cfg(windows)]
                {
                    [providers.detector, providers.encoder].iter().any(|provider| {
                        matches!(provider, OnnxProvider::DirectMl { device_id, .. } if *device_id < 0)
                    })
                }
                #[cfg(target_os = "macos")]
                {
                    false
                }
            }
        {
            return Err(OnnxFaceError::InvalidContract);
        }
        let detector_path = model_path(
            manifest,
            detector_stage_id,
            PersonStageKind::FaceDetector,
            store_dir,
        )?;
        let encoder_path = model_path(
            manifest,
            encoder_stage_id,
            PersonStageKind::FaceEncoder,
            store_dir,
        )?;
        if !digest_matches(runtime_library, runtime_sha256)? {
            return Err(OnnxFaceError::RuntimeDigestMismatch);
        }
        #[cfg(windows)]
        if [providers.detector, providers.encoder]
            .iter()
            .any(|provider| matches!(provider, OnnxProvider::DirectMl { .. }))
        {
            let expected = directml_sha256.ok_or(OnnxFaceError::InvalidContract)?;
            let companion = runtime_library.with_file_name("DirectML.dll");
            if !digest_matches(&companion, expected)? {
                return Err(OnnxFaceError::RuntimeDigestMismatch);
            }
        }
        initialize_runtime(runtime_library)?;
        let detector = build_session(
            &detector_path,
            providers.detector,
            None,
            #[cfg(target_os = "macos")]
            coreml_cache_dir,
        )?;
        let encoder = build_session(
            &encoder_path,
            providers.encoder,
            Some("batch_size"),
            #[cfg(target_os = "macos")]
            coreml_cache_dir,
        )?;
        if detector.inputs().len() != 1
            || detector.outputs().len() != 9
            || encoder.inputs().len() != 1
            || encoder.outputs().len() != 1
        {
            return Err(OnnxFaceError::InvalidContract);
        }
        Ok(Self {
            detector,
            encoder,
            detector_canvas,
            providers,
        })
    }

    pub fn providers(&self) -> OnnxFaceProviders {
        self.providers
    }

    /// Returns nine raw SCRFD heads and their input transform. Decode and
    /// quality decisions remain separate and must preserve multiple faces.
    pub fn detect_raw(
        &mut self,
        image: &RgbImage,
    ) -> Result<(ScrfdGeometry, Vec<OnnxTensor>), OnnxFaceError> {
        let input = scrfd_input(image, self.detector_canvas)?;
        let side = self.detector_canvas as usize;
        let tensor = TensorRef::from_array_view(([1, 3, side, side], input.values.as_slice()))
            .map_err(|error| OnnxFaceError::Ort(error.to_string()))?;
        let output_names: Vec<_> = self
            .detector
            .outputs()
            .iter()
            .map(|output| output.name().to_owned())
            .collect();
        let outputs = self
            .detector
            .run(ort::inputs![&*tensor])
            .map_err(|error| OnnxFaceError::Ort(error.to_string()))?;
        let mut result = Vec::with_capacity(outputs.len());
        for index in 0..outputs.len() {
            let (shape, values) = outputs[index]
                .try_extract_tensor::<f32>()
                .map_err(|error| OnnxFaceError::Ort(error.to_string()))?;
            let stride = [8_usize, 16, 32][index % 3];
            let rows = 2 * (side / stride).pow(2);
            let columns = [1_i64, 4, 10][index / 3];
            if shape.as_ref() != [rows as i64, columns]
                || values.iter().any(|value| !value.is_finite())
            {
                return Err(OnnxFaceError::InvalidContract);
            }
            result.push(OnnxTensor {
                name: output_names[index].clone(),
                shape: shape.to_vec(),
                values: values.to_vec(),
            });
        }
        Ok((
            ScrfdGeometry {
                scale: input.scale,
                resized_width: input.resized_width,
                resized_height: input.resized_height,
                canvas_size: input.canvas_size,
            },
            result,
        ))
    }

    /// Decode all qualifying faces, keeping multiple people in the image.
    /// Thresholds are caller supplied because recall must be calibrated on
    /// unseen shoots; neither threshold confirms identity or face quality.
    pub fn detect_faces(
        &mut self,
        image: &RgbImage,
        score_threshold: f32,
        nms_threshold: f32,
    ) -> Result<Vec<FaceDetection>, OnnxFaceError> {
        let (geometry, heads) = self.detect_raw(image)?;
        decode_scrfd(geometry, &heads, score_threshold, nms_threshold)
    }

    /// Convert model-space detections into the rebuildable stage cache format.
    /// This does not publish results; the analysis task's fenced transaction
    /// must reobserve the source before committing them.
    pub fn detect_for_cache(
        &mut self,
        image: &RgbImage,
        source_revision: &str,
        producer_fingerprint: &str,
        score_threshold: f32,
        nms_threshold: f32,
    ) -> Result<Vec<DetectedPersonInstance>, OnnxFaceError> {
        let faces = self.detect_faces(image, score_threshold, nms_threshold)?;
        detections_to_cache(&faces, image, source_revision, producer_fingerprint)
    }

    /// Encode a cached detection against the same oriented analysis image.
    /// A manual box correction requires a fresh landmark alignment step.
    pub fn encode_detected_face(
        &mut self,
        image: &RgbImage,
        detection: &DetectedPersonInstance,
    ) -> Result<Vec<f32>, OnnxFaceError> {
        let points = detection
            .face_landmarks
            .ok_or(OnnxFaceError::InvalidContract)?;
        let mut pixels = [[0.0; 2]; 5];
        for (pixel, point) in pixels.iter_mut().zip(points) {
            pixel[0] = point[0] * image.width() as f32;
            pixel[1] = point[1] * image.height() as f32;
        }
        self.encode_face(image, pixels)
    }

    /// Return a 512-D L2-normalized identity feature for one supplied face.
    /// Calling this does not confirm the person's identity.
    pub fn encode_face(
        &mut self,
        image: &RgbImage,
        landmarks: [[f32; 2]; 5],
    ) -> Result<Vec<f32>, OnnxFaceError> {
        let values = adaface_input(image, landmarks)?;
        let tensor = TensorRef::from_array_view(([1, 3, 112, 112], values.as_slice()))
            .map_err(|error| OnnxFaceError::Ort(error.to_string()))?;
        let outputs = self
            .encoder
            .run(ort::inputs![&*tensor])
            .map_err(|error| OnnxFaceError::Ort(error.to_string()))?;
        let (shape, values) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|error| OnnxFaceError::Ort(error.to_string()))?;
        normalize_embedding(shape, values)
    }
}

fn detections_to_cache(
    faces: &[FaceDetection],
    image: &RgbImage,
    source_revision: &str,
    producer_fingerprint: &str,
) -> Result<Vec<DetectedPersonInstance>, OnnxFaceError> {
    let width = f64::from(image.width());
    let height = f64::from(image.height());
    if width == 0.0
        || height == 0.0
        || faces.len() > 1000
        || source_revision.len() != 64
        || !source_revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        || producer_fingerprint.len() != 64
        || !producer_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(OnnxFaceError::InvalidContract);
    }
    let mut cached = Vec::with_capacity(faces.len());
    let mut seen_ids = HashSet::new();
    for face in faces {
        if !face.score.is_finite()
            || !(0.0..=1.0).contains(&face.score)
            || face.box_xyxy.iter().any(|value| !value.is_finite())
            || face
                .landmarks
                .iter()
                .flatten()
                .any(|value| !value.is_finite())
        {
            return Err(OnnxFaceError::InvalidContract);
        }
        let [x1, y1, x2, y2] = face.box_xyxy.map(f64::from);
        let x1 = x1.clamp(0.0, width);
        let y1 = y1.clamp(0.0, height);
        let x2 = x2.clamp(0.0, width);
        let y2 = y2.clamp(0.0, height);
        if x2 <= x1 || y2 <= y1 {
            continue;
        }
        let face_box = [
            (x1 / width),
            (y1 / height),
            (x2 - x1) / width,
            (y2 - y1) / height,
        ];
        let instance_id =
            detection_cache_id(source_revision, producer_fingerprint, Some(face_box), None)
                .map_err(|_| OnnxFaceError::InvalidContract)?;
        if !seen_ids.insert(instance_id.clone()) {
            continue;
        }
        let mut landmarks = face.landmarks;
        for point in &mut landmarks {
            point[0] /= image.width() as f32;
            point[1] /= image.height() as f32;
        }
        cached.push(DetectedPersonInstance {
            instance_id,
            face_box: Some(face_box),
            face_landmarks: Some(landmarks),
            body_box: None,
            face_score: Some(face.score),
            body_score: None,
            association_score: None,
        });
    }
    Ok(cached)
}

fn decode_scrfd(
    geometry: ScrfdGeometry,
    heads: &[OnnxTensor],
    score_threshold: f32,
    nms_threshold: f32,
) -> Result<Vec<FaceDetection>, OnnxFaceError> {
    if heads.len() != 9
        || !(320..=1280).contains(&geometry.canvas_size)
        || geometry.canvas_size & 31 != 0
        || !geometry.scale.is_finite()
        || geometry.scale <= 0.0
        || !score_threshold.is_finite()
        || !(0.0..=1.0).contains(&score_threshold)
        || !nms_threshold.is_finite()
        || !(0.0..=1.0).contains(&nms_threshold)
    {
        return Err(OnnxFaceError::InvalidContract);
    }
    let side = geometry.canvas_size as usize;
    let mut candidates = Vec::new();
    for (level, stride) in [8_usize, 16, 32].into_iter().enumerate() {
        let width = side / stride;
        let rows = 2 * width * width;
        let score = &heads[level];
        let bbox = &heads[level + 3];
        let kps = &heads[level + 6];
        for (head, columns) in [(score, 1), (bbox, 4), (kps, 10)] {
            if head.shape != [rows as i64, columns]
                || head.values.len() != rows * columns as usize
                || head.values.iter().any(|value| !value.is_finite())
            {
                return Err(OnnxFaceError::InvalidContract);
            }
        }
        if score
            .values
            .iter()
            .any(|value| !(0.0..=1.0).contains(value))
        {
            return Err(OnnxFaceError::InvalidContract);
        }
        for row in 0..rows {
            let confidence = score.values[row];
            if confidence < score_threshold {
                continue;
            }
            // SCRFD has two anchors at each grid cell, in raster order.
            let center_x = ((row / 2) % width * stride) as f32;
            let center_y = ((row / 2) / width * stride) as f32;
            let distances = &bbox.values[row * 4..row * 4 + 4];
            let multiplier = stride as f32 / geometry.scale;
            let x = center_x / geometry.scale;
            let y = center_y / geometry.scale;
            let box_xyxy = [
                x - distances[0] * multiplier,
                y - distances[1] * multiplier,
                x + distances[2] * multiplier,
                y + distances[3] * multiplier,
            ];
            if box_xyxy.iter().any(|value| !value.is_finite())
                || box_xyxy[2] <= box_xyxy[0]
                || box_xyxy[3] <= box_xyxy[1]
            {
                continue;
            }
            let mut landmarks = [[0.0; 2]; 5];
            for (index, point) in landmarks.iter_mut().enumerate() {
                point[0] = x + kps.values[row * 10 + index * 2] * multiplier;
                point[1] = y + kps.values[row * 10 + index * 2 + 1] * multiplier;
            }
            if landmarks.iter().flatten().any(|value| !value.is_finite()) {
                continue;
            }
            candidates.push(FaceDetection {
                box_xyxy,
                landmarks,
                score: confidence,
            });
        }
    }
    candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
    let mut kept = Vec::new();
    for candidate in candidates {
        if kept
            .iter()
            .all(|previous| scrfd_iou(previous, &candidate) <= nms_threshold)
        {
            kept.push(candidate);
        }
    }
    Ok(kept)
}

fn scrfd_iou(left: &FaceDetection, right: &FaceDetection) -> f32 {
    let area = |face: &FaceDetection| {
        let [x1, y1, x2, y2] = face.box_xyxy;
        (x2 - x1 + 1.0) * (y2 - y1 + 1.0)
    };
    let [ax1, ay1, ax2, ay2] = left.box_xyxy;
    let [bx1, by1, bx2, by2] = right.box_xyxy;
    let width = (ax2.min(bx2) - ax1.max(bx1) + 1.0).max(0.0);
    let height = (ay2.min(by2) - ay1.max(by1) + 1.0).max(0.0);
    let overlap = width * height;
    overlap / (area(left) + area(right) - overlap)
}

fn normalize_embedding(shape: &[i64], values: &[f32]) -> Result<Vec<f32>, OnnxFaceError> {
    if shape != [1, EMBEDDING_DIM as i64]
        || values.len() != EMBEDDING_DIM
        || values.iter().any(|value| !value.is_finite())
    {
        return Err(OnnxFaceError::InvalidContract);
    }
    let squared: f64 = values.iter().map(|value| f64::from(*value).powi(2)).sum();
    if !squared.is_finite() || squared <= f64::EPSILON {
        return Err(OnnxFaceError::InvalidContract);
    }
    let inverse = squared.sqrt().recip() as f32;
    Ok(values.iter().map(|value| value * inverse).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use image::Rgb;
    #[cfg(windows)]
    use std::time::Instant;

    #[test]
    fn runtime_hash_and_embedding_contract_are_checked() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().join("onnxruntime.dll");
        std::fs::write(&runtime, b"runtime fixture").unwrap();
        let correct = format!("{:x}", Sha256::digest(b"runtime fixture"));
        assert!(digest_matches(&runtime, &correct).unwrap());
        assert!(!digest_matches(&runtime, &"0".repeat(64)).unwrap());
        let values = vec![1.0; EMBEDDING_DIM];
        let normalized = normalize_embedding(&[1, 512], &values).unwrap();
        assert!((normalized.iter().map(|value| value * value).sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(normalize_embedding(&[512], &values).is_err());
        assert!(normalize_embedding(&[1, 512], &vec![0.0; EMBEDDING_DIM]).is_err());
    }

    #[test]
    fn scrfd_decodes_two_anchors_and_suppresses_only_overlapping_faces() {
        let side = 320_usize;
        let mut heads = Vec::new();
        for (columns, stride) in [
            (1, 8),
            (1, 16),
            (1, 32),
            (4, 8),
            (4, 16),
            (4, 32),
            (10, 8),
            (10, 16),
            (10, 32),
        ] {
            let rows = 2 * (side / stride).pow(2);
            heads.push(OnnxTensor {
                name: String::new(),
                shape: vec![rows as i64, columns],
                values: vec![0.0; rows * columns as usize],
            });
        }
        // Two anchors on the same grid center and a separate third face.
        for (row, confidence) in [(0, 0.9), (1, 0.8), (10, 0.7)] {
            heads[0].values[row] = confidence;
            heads[3].values[row * 4..row * 4 + 4].copy_from_slice(&[1.0; 4]);
            heads[6].values[row * 10..row * 10 + 10].copy_from_slice(&[0.5; 10]);
        }
        let geometry = ScrfdGeometry {
            scale: 2.0,
            resized_width: 320,
            resized_height: 320,
            canvas_size: 320,
        };
        let result = decode_scrfd(geometry, &heads, 0.5, 0.4).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].score, 0.9);
        assert_eq!(result[0].box_xyxy, [-4.0, -4.0, 4.0, 4.0]);
        assert_eq!(result[0].landmarks[0], [2.0, 2.0]);
        assert_eq!(result[1].score, 0.7);
        assert_eq!(result[1].box_xyxy, [16.0, -4.0, 24.0, 4.0]);
        assert!(decode_scrfd(geometry, &heads, f32::NAN, 0.4).is_err());
    }

    #[test]
    fn face_cache_bridge_clips_boxes_and_normalizes_landmarks() {
        let image = RgbImage::new(100, 50);
        let faces = [FaceDetection {
            box_xyxy: [-10.0, 5.0, 30.0, 45.0],
            landmarks: [[10.0, 12.0]; 5],
            score: 0.9,
        }];
        let cached = detections_to_cache(&faces, &image, &"a".repeat(64), &"b".repeat(64)).unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].face_box, Some([0.0, 0.1, 0.3, 0.8]));
        assert_eq!(cached[0].face_landmarks, Some([[0.1, 0.24]; 5]));
        assert_eq!(cached[0].instance_id.len(), 64);
        assert!(detections_to_cache(&faces, &image, "bad", &"b".repeat(64)).is_err());
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "requires an explicit Windows ML runtime and local research model files"]
    fn research_models_run_through_preprocessing_and_onnx_sessions() {
        let runtime = PathBuf::from(std::env::var_os("OXY_TEST_WINML_DLL").unwrap());
        let detector = PathBuf::from(std::env::var_os("OXY_TEST_SCRFD_ONNX").unwrap());
        let encoder = PathBuf::from(std::env::var_os("OXY_TEST_ADAFACE_ONNX").unwrap());
        let provider = match std::env::var("OXY_TEST_WINML_PROVIDER").as_deref() {
            Ok("dml") => OnnxProvider::DirectMl {
                device_id: 0,
                intra_threads: 4,
            },
            _ => OnnxProvider::Cpu { intra_threads: 4 },
        };
        initialize_runtime(&runtime).unwrap();
        let mut models = OnnxFaceModels {
            detector: build_session(&detector, provider, None).unwrap(),
            encoder: build_session(&encoder, provider, Some("batch_size")).unwrap(),
            detector_canvas: 640,
            providers: OnnxFaceProviders {
                detector: provider,
                encoder: provider,
            },
        };
        let image = RgbImage::from_pixel(640, 640, Rgb([128, 96, 64]));
        let (transform, outputs) = models.detect_raw(&image).unwrap();
        assert_eq!(transform.canvas_size, 640);
        assert_eq!(outputs.len(), 9);
        assert_eq!(outputs[0].shape, [12_800, 1]);
        assert_eq!(outputs[8].shape, [800, 10]);
        let detections = decode_scrfd(transform, &outputs, 0.5, 0.4).unwrap();
        assert!(detections.iter().all(|face| face.score >= 0.5));
        let cached = models
            .detect_for_cache(&image, &"a".repeat(64), &"b".repeat(64), 0.5, 0.4)
            .unwrap();
        assert!(cached.len() <= detections.len());
        let landmarks = [
            [250.0, 270.0],
            [390.0, 270.0],
            [320.0, 340.0],
            [265.0, 415.0],
            [375.0, 415.0],
        ];
        let embedding = models.encode_face(&image, landmarks).unwrap();
        assert_eq!(embedding.len(), 512);
        assert!((embedding.iter().map(|value| value * value).sum::<f32>() - 1.0).abs() < 1e-4);
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "manual release benchmark; requires explicit local runtime, models and JPEG"]
    fn benchmark_real_jpeg_face_stages() {
        let runtime = PathBuf::from(std::env::var_os("OXY_TEST_WINML_DLL").unwrap());
        let detector = PathBuf::from(std::env::var_os("OXY_TEST_SCRFD_ONNX").unwrap());
        let encoder = PathBuf::from(std::env::var_os("OXY_TEST_ADAFACE_ONNX").unwrap());
        let image_path = PathBuf::from(std::env::var_os("OXY_TEST_FACE_IMAGE").unwrap());
        let provider = match std::env::var("OXY_TEST_WINML_PROVIDER").as_deref() {
            Ok("dml") => OnnxProvider::DirectMl {
                device_id: 0,
                intra_threads: 4,
            },
            _ => OnnxProvider::Cpu { intra_threads: 4 },
        };
        initialize_runtime(&runtime).unwrap();
        let mut models = OnnxFaceModels {
            detector: build_session(&detector, provider, None).unwrap(),
            encoder: build_session(&encoder, provider, Some("batch_size")).unwrap(),
            detector_canvas: 640,
            providers: OnnxFaceProviders {
                detector: provider,
                encoder: provider,
            },
        };
        let started = Instant::now();
        let image = image::open(&image_path).unwrap().to_rgb8();
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
        let edge = image.width().max(image.height());
        let (width, height) = if edge > 1800 {
            (
                (u64::from(image.width()) * 1800 / u64::from(edge)) as u32,
                (u64::from(image.height()) * 1800 / u64::from(edge)) as u32,
            )
        } else {
            image.dimensions()
        };
        let started = Instant::now();
        let image =
            image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle);
        let resize_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut detect_ms = Vec::new();
        let mut align_ms = Vec::new();
        let mut encode_ms = Vec::new();
        let mut face_count = 0;
        for iteration in 0..11 {
            let started = Instant::now();
            let faces = models.detect_faces(&image, 0.5, 0.4).unwrap();
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            if iteration > 0 {
                detect_ms.push(elapsed);
            }
            face_count = faces.len();
            if let Some(face) = faces.first() {
                let started = Instant::now();
                let aligned = adaface_input(&image, face.landmarks).unwrap();
                assert_eq!(aligned.len(), 3 * 112 * 112);
                if iteration > 0 {
                    align_ms.push(started.elapsed().as_secs_f64() * 1000.0);
                }
                let started = Instant::now();
                let feature = models.encode_face(&image, face.landmarks).unwrap();
                assert_eq!(feature.len(), 512);
                if iteration > 0 {
                    encode_ms.push(started.elapsed().as_secs_f64() * 1000.0);
                }
            }
        }
        detect_ms.sort_by(f64::total_cmp);
        align_ms.sort_by(f64::total_cmp);
        encode_ms.sort_by(f64::total_cmp);
        eprintln!(
            "provider={provider:?} jpeg={} decoded={}x{} analysis={}x{} faces={} decode_ms={decode_ms:.2} resize_ms={resize_ms:.2} detect_median_ms={:.2} align_median_ms={:?} encode_median_ms={:?}",
            image_path.display(),
            image::image_dimensions(&image_path).unwrap().0,
            image::image_dimensions(&image_path).unwrap().1,
            width,
            height,
            face_count,
            detect_ms[5],
            align_ms.get(align_ms.len() / 2),
            encode_ms.get(encode_ms.len() / 2),
        );
    }
}
