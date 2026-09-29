//! Application-pinned research pipeline. The third-party WebFace12M ONNX was
//! compared with author-published WebFace12M weights; see the dated source audit.

use oxy_domain::{
    PersonDistanceMetric, PersonModelArtifact, PersonModelArtifactFormat, PersonModelLicenseStatus,
    PersonModelStatus, PersonStageKind, PersonStageManifest, PipelineManifest,
};
use std::path::Path;

pub const DETECTOR: &str = "scrfd-10g";
pub const ENCODER: &str = "adaface-webface12m";
pub const ENCODER_SOURCE: &str = "https://github.com/mk-minchul/AdaFace";

/// One frame shared by detection and face crops. Media chooses the representation.
pub const INPUT_REQUIREMENT: oxy_domain::AnalysisRequirement = oxy_domain::AnalysisRequirement {
    minimum_source_long_edge: 112,
    target_long_edge: 1600,
};

pub const ENCODER_DOWNLOAD: &str =
    "https://github.com/yakhyo/adaface-onnx/releases/download/weights/adaface_ir_101.onnx";

pub fn manifest() -> PipelineManifest {
    let artifact = |id: &str, url: &str, size, sha: &str, license: &str, license_url: &str| {
        PersonModelArtifact {
            artifact_id: id.into(),
            download_url: url.into(),
            format: PersonModelArtifactFormat::Onnx,
            size_bytes: size,
            sha256: sha.into(),
            license_name: license.into(),
            license_url: license_url.into(),
            license_status: PersonModelLicenseStatus::ResearchOnly,
        }
    };
    let stage = |id: &str, kind, depends_on, feature_space_id, artifact| PersonStageManifest {
        stage_id: id.into(),
        kind,
        depends_on,
        input_schema: "media-oriented-rgb-full-frame-1600-v5-simd-triangle".into(),
        output_schema: if id == DETECTOR {
            "scrfd-box-five-points-v1"
        } else {
            "normalized-512-v1"
        }
        .into(),
        runtime_version: if cfg!(windows) {
            "ort-directml-1.24.4-api24"
        } else {
            "ort-api27"
        }
        .into(),
        preprocessing_version: if id == DETECTOR {
            "scrfd-960-rgb-pad-v1"
        } else {
            "adaface-bgr-112-five-point-v1"
        }
        .into(),
        postprocessing_version: if id == DETECTOR {
            "score-0.5-nms-0.4-v1"
        } else {
            "l2-v1"
        }
        .into(),
        distance_metric: if id == ENCODER {
            Some(PersonDistanceMetric::Cosine)
        } else {
            None
        },
        feature_space_id,
        calibration_id: None,
        artifact: Some(artifact),
    };
    PipelineManifest {
        pipeline_id: "adaface-webface12m-face-analysis".into(),
        pipeline_version: if cfg!(windows) { "6" } else { "5" }.into(),
        stages: vec![
            stage(
                DETECTOR,
                PersonStageKind::FaceDetector,
                vec![],
                None,
                artifact(
                    DETECTOR,
                    "https://github.com/yakhyo/adaface-onnx/releases/download/weights/det_10g.onnx",
                    16923827,
                    "5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91",
                    "InsightFace pretrained models: non-commercial research",
                    "https://github.com/deepinsight/insightface",
                ),
            ),
            stage(
                ENCODER,
                PersonStageKind::FaceEncoder,
                vec![DETECTOR.into()],
                Some(
                    if cfg!(windows) {
                        "adaface-webface12m-f2eb07d0-media-v5-ort-dml-v1"
                    } else {
                        "adaface-webface12m-f2eb07d0-media-v5"
                    }
                    .into(),
                ),
                artifact(
                    ENCODER,
                    ENCODER_DOWNLOAD,
                    260704652,
                    "f2eb07d03de0af560a82e1214df799fec5e09375d43521e2868f9dc387e5a43e",
                    "AdaFace official weights: non-commercial research",
                    ENCODER_SOURCE,
                ),
            ),
        ],
    }
}

pub fn model_status(store: &Path) -> Vec<PersonModelStatus> {
    let manifest = manifest();
    manifest
        .stages
        .iter()
        .filter_map(|stage| stage.artifact.as_ref())
        .map(|artifact| {
            let installed = crate::environment::artifacts::installed_artifact(
                &manifest,
                &artifact.artifact_id,
                store,
            );
            PersonModelStatus {
                id: artifact.artifact_id.clone(),
                name: if artifact.artifact_id == DETECTOR {
                    "SCRFD 人脸检测"
                } else {
                    "AdaFace WebFace12M 人脸特征"
                }
                .into(),
                size_bytes: artifact.size_bytes,
                installed: matches!(installed, Ok(Some(_))),
                download_available: true,
                source_url: "https://github.com/yakhyo/adaface-onnx".into(),
                usage: if artifact.artifact_id == ENCODER {
                    "官方训练权重的第三方 ONNX 导出，已核对同源输出；仅限非商业研究。"
                } else {
                    "InsightFace 检测模型的第三方镜像；仅限非商业研究。"
                }
                .into(),
                error: installed.err().map(|error| error.to_string()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn media_contract_upgrade_can_write_beside_existing_v1_features() {
        let people = crate::testing::in_memory();
        let current = super::manifest();
        let mut previous = current.clone();
        previous.pipeline_version = "1".into();
        for stage in &mut previous.stages {
            stage.input_schema = "oriented-rgb-v1".into();
            if stage.stage_id == super::ENCODER {
                stage.feature_space_id = Some("adaface-webface12m-f2eb07d0-v1".into());
            }
        }
        let folder = tempfile::tempdir().unwrap();
        let mut spaces = Vec::new();
        for manifest in [&previous, &current] {
            let space = manifest
                .stages
                .iter()
                .find(|stage| stage.stage_id == super::ENCODER)
                .unwrap()
                .feature_space_id
                .clone()
                .unwrap();
            let mut values = vec![0.0; 512];
            values[0] = 1.0;
            people
                .put_person_feature(&crate::features::PersonFeature {
                    folder_path: folder.path().into(),
                    asset_path: folder.path().join("image.jpg"),
                    instance_id: "face".into(),
                    source_revision: "unchanged-source".into(),
                    feature_space_id: space.clone(),
                    modality: oxy_domain::FeatureModality::Face,
                    producer_fingerprint: crate::stage_fingerprint(manifest, super::ENCODER)
                        .unwrap(),
                    pipeline_fingerprint: crate::pipeline_fingerprint(manifest).unwrap(),
                    values,
                })
                .unwrap();
            spaces.push(space);
        }
        assert_ne!(spaces[0], spaces[1]);
        for space in spaces {
            let mut query = vec![0.0; 512];
            query[0] = 1.0;
            let matches = people
                .search_person_features(
                    folder.path(),
                    &space,
                    &query,
                    0.9,
                    (10, 0),
                    &[(folder.path().join("image.jpg"), "unchanged-source".into())],
                )
                .unwrap();
            assert_eq!(
                matches.len(),
                1,
                "both versions retain independently queryable features"
            );
        }
    }

    #[test]
    fn pinned_manifest_is_installable_and_has_stable_stage_order() {
        let manifest = super::manifest();
        crate::require_installable(&manifest).unwrap();
        assert_eq!(
            crate::execution::enrollment::asset_stage_order(&manifest).unwrap(),
            [super::DETECTOR, super::ENCODER]
        );
    }

    #[test]
    #[cfg(windows)]
    fn standalone_runtime_uses_a_distinct_feature_space_from_winml() {
        let current = super::manifest();
        let encoder = current
            .stages
            .iter()
            .find(|stage| stage.stage_id == super::ENCODER)
            .unwrap();
        assert_ne!(
            encoder.feature_space_id.as_deref(),
            Some("adaface-webface12m-f2eb07d0-media-v4")
        );
        assert_eq!(encoder.runtime_version, "ort-directml-1.24.4-api24");
    }
}
