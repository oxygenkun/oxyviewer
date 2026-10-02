//! The person domain.
//!
//! Two halves meet here. One is what a person *is*: identity, the review
//! decisions over it, the history of both, the tag an identity pushes onto its
//! photos, and the reference a query is anchored on ([`People`]). The other is
//! the pipeline that proposes identities: manifest validation, stage
//! fingerprints, detection geometry, artifact verification, and the model
//! adapters that will consume them.
//!
//! The claims the boundary is supposed to make true:
//!
//! - **A person fact is never a cache.** Identity, reviews, references, and
//!   history are declared `user` in `oxy_store::schema`; detections, feature
//!   vectors, and the analysis run ledger are declared `cache`. Emptying the
//!   second never reaches the first, and that audit runs in `oxy_store`.
//! - **The domain does not name the storage engine.** Every statement is a
//!   function in `oxy_store::repo::person_cache`; this crate holds the rules.
//!   [`audit`] asserts both halves of that.
//! - **`oxy-people` is a sibling of `oxy-tags`, not a layer above it.** A
//!   person identity may push a tag onto its photos, so the two domains talk to
//!   each other's rows through repository statements rather than through each
//!   other's API. Nothing here names `oxy_tags`.

pub mod alignment;
pub mod clusters;
pub mod detections;
pub mod environment;
pub mod execution;
pub mod features;
mod geometry;
pub mod global_people;
pub mod identity;
pub mod inference;

#[cfg(test)]
mod audit;
#[cfg(test)]
mod testing;

use oxy_domain::{
    PersonModelLicenseStatus, PersonStageKind, PersonStageManifest, PipelineManifest,
};
use oxy_store::Store;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use thiserror::Error;

/// The person domain over one store.
///
/// It holds the store rather than opening one: the application opens the file
/// once and gives it to every domain crate, so none of them wraps another and
/// none of them decides where the database lives.
pub struct People {
    pub(crate) store: Arc<Store>,
    /// The vector extension's state, copied from the store at construction.
    ///
    /// It is kept here, rather than asked of the store on every call, so a test
    /// can state "this build has no vector support" without a broken library.
    pub(crate) vector_status: Result<String, String>,
}

impl People {
    pub fn new(store: Arc<Store>) -> Self {
        let vector_status = store.vector_status();
        Self {
            store,
            vector_status,
        }
    }
}

/// Failures from a person rule or from the storage it was applied to.
///
/// The engine's own failures stay inside [`oxy_store::StoreError`]; these are
/// the rules — a revision that moved, an identity claimed twice, a stage that
/// may not publish.
#[derive(Debug, Error)]
pub enum PeopleError {
    #[error("{0}")]
    Cluster(String),
    #[error(transparent)]
    Store(#[from] oxy_store::StoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Filesystem(#[from] oxy_fs::FsError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("tag parent does not exist")]
    MissingTagParent,
    #[error("person record changed; reload before saving")]
    PersonConflict,
    #[error("person record or instance was not found in this folder")]
    MissingPersonRecord,
    #[error("invalid person instance box or source revision")]
    InvalidPersonInstance,
    #[error("person vector search unavailable: {0}")]
    PersonVectorUnavailable(String),
    #[error("invalid person feature or feature-space contract")]
    InvalidPersonFeature,
    #[error("invalid automatic person instance or detection evidence")]
    InvalidPersonDetection,
    #[error("invalid person analysis request")]
    InvalidPersonAnalysis,
    #[error("person analysis run is missing")]
    MissingPersonAnalysis,
    #[error("person analysis request or generation changed")]
    PersonAnalysisConflict,
}

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("invalid pipeline identity or stage contract")]
    InvalidContract,
    #[error("duplicate stage or feature space")]
    DuplicateStage,
    #[error("missing dependency or dependency cycle")]
    InvalidDependency,
    #[error("model artifact lacks a valid trusted source or checksum")]
    InvalidArtifact,
    #[error("model artifact use terms are unresolved")]
    LicenseUnavailable,
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
}

fn nonempty(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

pub fn validate_manifest(manifest: &PipelineManifest) -> Result<(), PipelineError> {
    if !nonempty(&manifest.pipeline_id)
        || !nonempty(&manifest.pipeline_version)
        || manifest.stages.is_empty()
    {
        return Err(PipelineError::InvalidContract);
    }
    let mut stages = HashMap::new();
    let mut spaces = HashSet::new();
    let mut artifacts = HashMap::new();
    for stage in &manifest.stages {
        if !nonempty(&stage.stage_id)
            || !nonempty(&stage.input_schema)
            || !nonempty(&stage.output_schema)
            || !nonempty(&stage.runtime_version)
            || !nonempty(&stage.preprocessing_version)
            || !nonempty(&stage.postprocessing_version)
        {
            return Err(PipelineError::InvalidContract);
        }
        if stages.insert(stage.stage_id.as_str(), stage).is_some() {
            return Err(PipelineError::DuplicateStage);
        }
        match stage.kind {
            PersonStageKind::FaceEncoder | PersonStageKind::BodyEncoder => {
                let Some(space) = stage.feature_space_id.as_deref() else {
                    return Err(PipelineError::InvalidContract);
                };
                if !nonempty(space) || stage.distance_metric.is_none() {
                    return Err(PipelineError::InvalidContract);
                }
                if !spaces.insert(space) {
                    return Err(PipelineError::DuplicateStage);
                }
            }
            _ if stage.feature_space_id.is_some() => return Err(PipelineError::InvalidContract),
            _ => {}
        }
        if let Some(artifact) = &stage.artifact {
            if !nonempty(&artifact.artifact_id)
                || !artifact.download_url.starts_with("https://")
                || artifact.size_bytes == 0
                || artifact.sha256.len() != 64
                || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !nonempty(&artifact.license_name)
                || !artifact.license_url.starts_with("https://")
            {
                return Err(PipelineError::InvalidArtifact);
            }
            if let Some(previous) = artifacts.insert(artifact.artifact_id.as_str(), artifact)
                && previous != artifact
            {
                return Err(PipelineError::InvalidArtifact);
            }
        }
    }
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();
    fn visit<'a>(
        id: &'a str,
        stages: &HashMap<&'a str, &'a oxy_domain::PersonStageManifest>,
        visited: &mut HashSet<&'a str>,
        visiting: &mut HashSet<&'a str>,
    ) -> Result<(), PipelineError> {
        if visited.contains(id) {
            return Ok(());
        }
        if !visiting.insert(id) {
            return Err(PipelineError::InvalidDependency);
        }
        let stage = stages.get(id).ok_or(PipelineError::InvalidDependency)?;
        for dependency in &stage.depends_on {
            visit(dependency, stages, visited, visiting)?;
        }
        visiting.remove(id);
        visited.insert(id);
        Ok(())
    }
    for id in stages.keys() {
        visit(id, &stages, &mut visited, &mut visiting)?;
    }
    Ok(())
}

fn semantic_stage_fingerprint(
    stage_id: &str,
    stages: &HashMap<&str, &PersonStageManifest>,
    memo: &mut HashMap<String, String>,
) -> Result<String, PipelineError> {
    if let Some(cached) = memo.get(stage_id) {
        return Ok(cached.clone());
    }
    let stage = stages
        .get(stage_id)
        .ok_or(PipelineError::InvalidDependency)?;
    let mut dependencies = stage
        .depends_on
        .iter()
        .map(|dependency| {
            Ok((
                dependency.as_str(),
                semantic_stage_fingerprint(dependency, stages, memo)?,
            ))
        })
        .collect::<Result<Vec<_>, PipelineError>>()?;
    dependencies.sort_by_key(|(id, _)| *id);
    let artifact = stage.artifact.as_ref().map(|artifact| {
        serde_json::json!({
            "sha256": artifact.sha256.to_ascii_lowercase(),
            "format": artifact.format,
        })
    });
    let semantic = serde_json::json!({
        "stageId": stage.stage_id,
        "kind": stage.kind,
        "dependencies": dependencies,
        "inputSchema": stage.input_schema,
        "outputSchema": stage.output_schema,
        "runtimeVersion": stage.runtime_version,
        "preprocessingVersion": stage.preprocessing_version,
        "postprocessingVersion": stage.postprocessing_version,
        "featureSpaceId": stage.feature_space_id,
        "distanceMetric": stage.distance_metric,
        "calibrationId": stage.calibration_id,
        "artifact": artifact,
    });
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&semantic)?));
    memo.insert(stage_id.to_owned(), fingerprint.clone());
    Ok(fingerprint)
}

/// Hashes one stage and only its transitive dependencies. Download locations
/// and license metadata do not change inference output and are excluded.
pub fn stage_fingerprint(
    manifest: &PipelineManifest,
    stage_id: &str,
) -> Result<String, PipelineError> {
    validate_manifest(manifest)?;
    let stages = manifest
        .stages
        .iter()
        .map(|stage| (stage.stage_id.as_str(), stage))
        .collect();
    semantic_stage_fingerprint(stage_id, &stages, &mut HashMap::new())
}

pub fn pipeline_fingerprint(manifest: &PipelineManifest) -> Result<String, PipelineError> {
    validate_manifest(manifest)?;
    let stages: HashMap<_, _> = manifest
        .stages
        .iter()
        .map(|stage| (stage.stage_id.as_str(), stage))
        .collect();
    let mut stage_ids: Vec<_> = stages.keys().copied().collect();
    stage_ids.sort_unstable();
    let mut memo = HashMap::new();
    let fingerprints = stage_ids
        .into_iter()
        .map(|id| Ok((id, semantic_stage_fingerprint(id, &stages, &mut memo)?)))
        .collect::<Result<Vec<_>, PipelineError>>()?;
    let semantic = serde_json::json!({
        "pipelineId": manifest.pipeline_id,
        "pipelineVersion": manifest.pipeline_version,
        "stages": fingerprints,
    });
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&semantic)?)
    ))
}

/// Stages that must be produced in `next` because their own semantics or an
/// upstream dependency changed. Returned IDs are sorted for stable UI output.
pub fn stages_to_recompute(
    previous: &PipelineManifest,
    next: &PipelineManifest,
) -> Result<Vec<String>, PipelineError> {
    validate_manifest(previous)?;
    validate_manifest(next)?;
    let old_ids: HashSet<_> = previous
        .stages
        .iter()
        .map(|stage| stage.stage_id.as_str())
        .collect();
    let mut changed = Vec::new();
    for stage in &next.stages {
        if !old_ids.contains(stage.stage_id.as_str())
            || stage_fingerprint(previous, &stage.stage_id)?
                != stage_fingerprint(next, &stage.stage_id)?
        {
            changed.push(stage.stage_id.clone());
        }
    }
    changed.sort_unstable();
    Ok(changed)
}

pub fn require_installable(manifest: &PipelineManifest) -> Result<(), PipelineError> {
    validate_manifest(manifest)?;
    const REQUIRED: [PersonStageKind; 8] = [
        PersonStageKind::FaceDetector,
        PersonStageKind::BodyDetector,
        PersonStageKind::Association,
        PersonStageKind::QualityGate,
        PersonStageKind::FaceEncoder,
        PersonStageKind::BodyEncoder,
        PersonStageKind::Clusterer,
        PersonStageKind::Retriever,
    ];
    let face_only = manifest.stages.len() == 2 && {
        let detector = manifest
            .stages
            .iter()
            .find(|stage| stage.kind == PersonStageKind::FaceDetector);
        let encoder = manifest
            .stages
            .iter()
            .find(|stage| stage.kind == PersonStageKind::FaceEncoder);
        matches!((detector, encoder), (Some(detector), Some(encoder))
            if detector.depends_on.is_empty() && encoder.depends_on == [detector.stage_id.clone()])
    };
    if !face_only
        && REQUIRED
            .iter()
            .any(|kind| !manifest.stages.iter().any(|stage| stage.kind == *kind))
    {
        return Err(PipelineError::InvalidContract);
    }
    if manifest.stages.iter().any(|stage| {
        matches!(
            stage.kind,
            PersonStageKind::FaceDetector
                | PersonStageKind::BodyDetector
                | PersonStageKind::FaceEncoder
                | PersonStageKind::BodyEncoder
        ) && stage.artifact.is_none()
    }) {
        return Err(PipelineError::InvalidArtifact);
    }
    if manifest
        .stages
        .iter()
        .filter_map(|stage| stage.artifact.as_ref())
        .any(|artifact| artifact.license_status == PersonModelLicenseStatus::Unresolved)
    {
        return Err(PipelineError::LicenseUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::artifacts::{
        ArtifactInstallError, install_artifact_reader, install_local_artifact,
    };
    use oxy_domain::{
        PersonDistanceMetric, PersonModelArtifact, PersonModelArtifactFormat, PersonStageManifest,
    };

    fn manifest() -> PipelineManifest {
        let kinds = [
            ("face-detector", PersonStageKind::FaceDetector),
            ("body-detector", PersonStageKind::BodyDetector),
            ("association", PersonStageKind::Association),
            ("quality", PersonStageKind::QualityGate),
            ("face-encoder", PersonStageKind::FaceEncoder),
            ("body-encoder", PersonStageKind::BodyEncoder),
            ("clusterer", PersonStageKind::Clusterer),
            ("retriever", PersonStageKind::Retriever),
        ];
        let stages = kinds
            .iter()
            .enumerate()
            .map(|(index, (id, kind))| {
                let is_encoder = matches!(
                    kind,
                    PersonStageKind::FaceEncoder | PersonStageKind::BodyEncoder
                );
                let uses_model = matches!(
                    kind,
                    PersonStageKind::FaceDetector
                        | PersonStageKind::BodyDetector
                        | PersonStageKind::FaceEncoder
                        | PersonStageKind::BodyEncoder
                );
                PersonStageManifest {
                    stage_id: (*id).into(),
                    kind: *kind,
                    depends_on: if index == 0 {
                        Vec::new()
                    } else {
                        vec![kinds[index - 1].0.into()]
                    },
                    input_schema: "input-v1".into(),
                    output_schema: "output-v1".into(),
                    runtime_version: "ort-1".into(),
                    preprocessing_version: "bgr-0.5-v1".into(),
                    postprocessing_version: "l2-v1".into(),
                    feature_space_id: is_encoder.then(|| format!("{id}-test-f3")),
                    distance_metric: is_encoder.then_some(PersonDistanceMetric::Cosine),
                    calibration_id: is_encoder.then(|| "test-v1".into()),
                    artifact: uses_model.then(|| PersonModelArtifact {
                        artifact_id: (*id).into(),
                        download_url: format!("https://example.org/{id}.onnx"),
                        format: PersonModelArtifactFormat::Onnx,
                        size_bytes: 123,
                        sha256: "a".repeat(64),
                        license_name: "Example".into(),
                        license_url: "https://example.org/license".into(),
                        license_status: PersonModelLicenseStatus::VerifiedForDistribution,
                    }),
                }
            })
            .collect();
        PipelineManifest {
            pipeline_id: "test-face".into(),
            pipeline_version: "1".into(),
            stages,
        }
    }

    #[test]
    fn fingerprints_change_with_preprocessing_and_reject_unresolved_models() {
        let initial = manifest();
        let first = pipeline_fingerprint(&initial).unwrap();
        let face_before = stage_fingerprint(&initial, "face-encoder").unwrap();
        let retrieval_before = stage_fingerprint(&initial, "retriever").unwrap();
        require_installable(&initial).unwrap();
        let mut changed = initial;
        changed.stages[0].preprocessing_version = "rgb-v2".into();
        assert_ne!(first, pipeline_fingerprint(&changed).unwrap());
        changed.stages[0].artifact.as_mut().unwrap().license_status =
            PersonModelLicenseStatus::ResearchOnly;
        require_installable(&changed).unwrap();
        changed.stages[0].artifact.as_mut().unwrap().license_status =
            PersonModelLicenseStatus::Unresolved;
        assert!(matches!(
            require_installable(&changed),
            Err(PipelineError::LicenseUnavailable)
        ));
        changed.stages[0].artifact.as_mut().unwrap().download_url =
            "http://example.org/weights".into();
        assert!(matches!(
            validate_manifest(&changed),
            Err(PipelineError::InvalidArtifact)
        ));
        let mut unrelated = manifest();
        unrelated.stages[5].preprocessing_version = "body-v2".into();
        assert_eq!(
            stages_to_recompute(&manifest(), &unrelated).unwrap(),
            ["body-encoder", "clusterer", "retriever"]
        );
        assert_eq!(
            face_before,
            stage_fingerprint(&unrelated, "face-encoder").unwrap()
        );
        assert_ne!(
            retrieval_before,
            stage_fingerprint(&unrelated, "retriever").unwrap()
        );
        let mut moved = manifest();
        moved.stages.reverse();
        assert_eq!(first, pipeline_fingerprint(&moved).unwrap());
        let mut provenance_only = manifest();
        provenance_only.stages[0]
            .artifact
            .as_mut()
            .unwrap()
            .download_url = "https://mirror.example.org/weights.onnx".into();
        assert_eq!(first, pipeline_fingerprint(&provenance_only).unwrap());
        let mut incomplete = manifest();
        incomplete.stages.pop();
        assert!(matches!(
            require_installable(&incomplete),
            Err(PipelineError::InvalidContract)
        ));
        let mut missing_weights = manifest();
        missing_weights.stages[0].artifact = None;
        assert!(matches!(
            require_installable(&missing_weights),
            Err(PipelineError::InvalidArtifact)
        ));
    }

    #[test]
    fn rejects_cycles_and_duplicate_feature_spaces() {
        let mut pipeline = manifest();
        pipeline.stages[0].depends_on.push("face-encoder".into());
        assert!(matches!(
            validate_manifest(&pipeline),
            Err(PipelineError::InvalidDependency)
        ));
        pipeline.stages[0].depends_on.clear();
        let mut second = pipeline.stages[4].clone();
        second.stage_id = "face-encoder-two".into();
        pipeline.stages.push(second);
        assert!(matches!(
            validate_manifest(&pipeline),
            Err(PipelineError::DuplicateStage)
        ));
        let mut shared_artifact = manifest();
        let mut another_detector = shared_artifact.stages[0].clone();
        another_detector.stage_id = "face-detector-alt".into();
        shared_artifact.stages.push(another_detector);
        validate_manifest(&shared_artifact).unwrap();
        shared_artifact
            .stages
            .last_mut()
            .unwrap()
            .artifact
            .as_mut()
            .unwrap()
            .sha256 = "b".repeat(64);
        assert!(matches!(
            validate_manifest(&shared_artifact),
            Err(PipelineError::InvalidArtifact)
        ));
    }

    #[test]
    fn local_artifact_import_checks_length_hash_and_existing_content() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("download.part");
        let store = temporary.path().join("models");
        let bytes = b"verified model file";
        let mut pipeline = manifest();
        let artifact = pipeline.stages[0].artifact.as_mut().unwrap();
        artifact.size_bytes = bytes.len() as u64;
        artifact.sha256 = format!("{:x}", Sha256::digest(bytes));
        artifact.license_status = PersonModelLicenseStatus::ResearchOnly;
        std::fs::write(&source, b"corrupt model file!").unwrap();
        assert!(matches!(
            install_local_artifact(&pipeline, "face-detector", &source, &store),
            Err(ArtifactInstallError::InvalidContent)
        ));
        assert_eq!(std::fs::read_dir(&store).unwrap().count(), 0);
        std::fs::write(&source, bytes).unwrap();
        let installed =
            install_local_artifact(&pipeline, "face-detector", &source, &store).unwrap();
        assert_eq!(std::fs::read(&installed).unwrap(), bytes);
        assert_eq!(
            install_local_artifact(&pipeline, "face-detector", &source, &store).unwrap(),
            installed
        );
        std::fs::write(&installed, b"corrupt").unwrap();
        assert!(matches!(
            install_local_artifact(&pipeline, "face-detector", &source, &store),
            Err(ArtifactInstallError::CorruptInstalledArtifact)
        ));
    }

    #[test]
    fn cancelled_artifact_stream_leaves_no_installed_or_partial_file() {
        let temporary = tempfile::tempdir().unwrap();
        let store = temporary.path().join("models");
        let bytes = vec![42_u8; 150_000];
        let mut pipeline = manifest();
        let artifact = pipeline.stages[0].artifact.as_mut().unwrap();
        artifact.size_bytes = bytes.len() as u64;
        artifact.sha256 = format!("{:x}", Sha256::digest(&bytes));
        let mut observations = Vec::new();
        let outcome = install_artifact_reader(
            &pipeline,
            "face-detector",
            std::io::Cursor::new(bytes),
            &store,
            &mut |completed, total| {
                observations.push((completed, total));
                completed == 0
            },
        );
        assert!(matches!(outcome, Err(ArtifactInstallError::Cancelled)));
        assert_eq!(observations[0].0, 0);
        assert!(observations.iter().any(|(completed, _)| *completed > 0));
        assert_eq!(std::fs::read_dir(&store).unwrap().count(), 0);
    }
}
