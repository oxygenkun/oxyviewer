//! One decode per asset, serial model calls, persistence on its own stage.
use super::{
    operations::PersonOperation,
    persistence::{FaceOutput, InferredAsset, save},
    pipeline,
};
use crate::{
    People, environment::catalog, features::PersonFeature, inference::face::OnnxFaceModels,
};
use oxy_domain::{
    AnalysisRequirement, FeatureModality, PersonAnalysisRun, PersonAnalysisTask, PipelineManifest,
};

struct PreparedAsset {
    task: PersonAnalysisTask,
    pixels: Result<image::RgbImage, String>,
}

pub(super) fn execute(
    people: &People,
    run: &PersonAnalysisRun,
    manifest: &PipelineManifest,
    models: &mut OnnxFaceModels,
    operation: &PersonOperation,
    mut prepare: impl FnMut(&PersonAnalysisTask, AnalysisRequirement) -> Result<image::RgbImage, String>
    + Send,
) -> Result<PersonAnalysisRun, String> {
    let encoder = manifest
        .stages
        .iter()
        .find(|stage| stage.stage_id == catalog::ENCODER)
        .ok_or("缺少编码阶段")?;
    let feature_space = encoder.feature_space_id.as_ref().ok_or("缺少特征空间")?;
    let fingerprint =
        crate::stage_fingerprint(manifest, catalog::ENCODER).map_err(|e| e.to_string())?;
    let backend = models.providers().detector.description();
    let result = pipeline::execute(
        || {
            if !people
                .person_analysis_is_current(&run.run_id)
                .map_err(|e| e.to_string())?
            {
                return Ok(None);
            }
            let Some(task) = people
                .claim_person_analysis_stage(&run.run_id, catalog::DETECTOR, None)
                .map_err(|e| e.to_string())?
            else {
                return Ok(None);
            };
            let pixels = prepare(&task, catalog::INPUT_REQUIREMENT).and_then(|pixels| {
                // Enforce the transport bound even for a faulty input adapter.
                if pixels.width().max(pixels.height()) > catalog::INPUT_REQUIREMENT.target_long_edge
                {
                    Err("分析输入超过流水线像素预算".into())
                } else {
                    Ok(pixels)
                }
            });
            Ok(Some(PreparedAsset { task, pixels }))
        },
        |prepared: PreparedAsset| {
            let task = prepared.task;
            operation.update(|s| {
                s.detail = format!(
                    "正在识别 {}（{backend}，流水线运行中）",
                    task.asset_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                );
            });
            let output = prepared.pixels.and_then(|pixels| {
                if operation.is_cancelled() {
                    return Err("已取消".into());
                }
                let detections = models
                    .detect_for_cache(
                        &pixels,
                        &task.source_revision,
                        &task.stage_fingerprint,
                        0.5,
                        0.4,
                    )
                    .map_err(|e| e.to_string())?;
                let features = (|| {
                    let mut features = Vec::with_capacity(detections.len());
                    for detection in &detections {
                        if operation.is_cancelled() {
                            return Err("已取消".into());
                        }
                        let values = models
                            .encode_detected_face(&pixels, detection)
                            .map_err(|e| e.to_string())?;
                        features.push(PersonFeature {
                            folder_path: task.folder_path.clone(),
                            asset_path: task.asset_path.clone(),
                            instance_id: detection.instance_id.clone(),
                            source_revision: task.source_revision.clone(),
                            feature_space_id: feature_space.clone(),
                            modality: FeatureModality::Face,
                            producer_fingerprint: fingerprint.clone(),
                            pipeline_fingerprint: run.pipeline_fingerprint.clone(),
                            values,
                        });
                    }
                    Ok(features)
                })();
                Ok(FaceOutput {
                    detections,
                    features,
                })
            });
            // pixels are dropped here, before a bounded result send can block.
            InferredAsset { task, output }
        },
        |asset| save(people, asset, catalog::ENCODER, operation),
        || operation.is_cancelled(),
    );
    if operation.is_cancelled() {
        return people
            .cancel_person_analysis(&run.run_id, &format!("worker-cancel:{}", run.run_id))
            .map_err(|e| e.to_string());
    }
    if !people
        .person_analysis_is_current(&run.run_id)
        .map_err(|e| e.to_string())?
    {
        return people
            .get_person_analysis(&run.run_id)
            .map_err(|e| e.to_string());
    }
    result?;
    Err("流水线已结束但仍有未完成的识别步骤".into())
}
