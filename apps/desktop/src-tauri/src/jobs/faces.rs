//! Background face analysis.
//!
//! Runs the analyzer over library assets on its own thread, one asset at a
//! time, and writes results through the library. Three properties matter more
//! than throughput here:
//!
//! * **Browsing always wins.** The worker waits on the library's foreground
//!   gate before each asset, so analysis can never make the grid or loupe
//!   slower. A face job is only ever a background consumer.
//! * **A run is resumable.** Every asset is checkpointed by the library
//!   (including assets with zero faces), so killing the job and starting again
//!   continues instead of re-decoding the library.
//! * **Stale results are rejected.** The source revision is read before the
//!   decode and verified again before the write, so replacing a file
//!   mid-analysis cannot record observations for bytes that no longer exist.
//!
//! A missing or broken model is a normal state, not a crash: the queue reports
//! `Failed` progress and the rest of the application is unaffected.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use oxy_domain::{
    FaceAnalysisProgress, FaceAnalysisRequest, FaceAnalysisStage, FaceCandidate, FaceCluster,
    FaceLibraryStats, PreviewPriority, RenderLevel,
};
use oxy_faces::{
    ClusterInput, FaceAnalyzer, FaceModelPaths, PersonGallery, RgbImage, cluster_embeddings,
    match_person,
};
use oxy_library::{FaceAnalysisTarget, Library};
use oxy_runtime::CancellationToken;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::cache::CacheManager;
use crate::state::people::PeopleService;

pub(crate) const FACE_ANALYSIS_PROGRESS_EVENT: &str = "face-analysis-progress";
pub(crate) const FACE_LIBRARY_UPDATED_EVENT: &str = "face-library-updated";
/// Bounds the clustering pass, which compares every pair of unknown faces.
const CLUSTER_LIMIT: usize = 20_000;
/// Bounds the candidate pass, which is linear in unknown faces.
const CANDIDATE_LIMIT: usize = 50_000;
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);

/// The two pinned model files and the detector's fixed input size.
///
/// These names and the `face-models` directory are also what
/// `3rdpart/face-models/prepare.mjs` stages into
/// `apps/desktop/src-tauri/resources/`, and what `tauri.bundle.json` maps to the
/// package's resource directory. Changing one without the others silently makes
/// the feature unavailable in packaged builds rather than failing loudly.
pub(crate) const DETECTOR_MODEL_FILE: &str = "face_detection_yunet_2023mar.onnx";
pub(crate) const EMBEDDER_MODEL_FILE: &str = "face_recognition_sface_2021dec.onnx";
pub(crate) const DETECTOR_INPUT_SIZE: u32 = 640;

/// Both model files under one directory, or `None` when either is missing.
fn model_paths_in(directory: &Path) -> Option<FaceModelPaths> {
    let detector = directory.join(DETECTOR_MODEL_FILE);
    let embedder = directory.join(EMBEDDER_MODEL_FILE);
    (detector.is_file() && embedder.is_file())
        .then(|| FaceModelPaths::new(detector, embedder, DETECTOR_INPUT_SIZE))
}

/// Resolves the pinned model files. A missing directory is a supported state.
pub(crate) fn resolve_model_paths(app: &AppHandle) -> Option<FaceModelPaths> {
    let mut candidates = Vec::new();
    if let Some(directory) = std::env::var_os("OXY_FACE_MODEL_DIR") {
        candidates.push(PathBuf::from(directory));
    }
    if let Ok(resource) = app.path().resource_dir() {
        // Where `tauri.bundle.json` places the staged models.
        candidates.push(resource.join("face-models"));
    }
    // Development fallback: `pnpm faces:prepare` writes into the workspace
    // target directory.
    candidates
        .push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/native/face-models"));

    candidates
        .iter()
        .find_map(|directory| model_paths_in(directory))
}

#[derive(Default)]
struct FaceWork {
    active: Option<ActiveRun>,
    progress: Option<FaceAnalysisProgress>,
    /// Requests arriving while a run is active are coalesced into one follow-up.
    queued: Option<FaceAnalysisRequest>,
}

struct ActiveRun {
    job_id: String,
    cancellation: CancellationToken,
}

/// Counters from one completed pass.
struct RunOutcome {
    processed: u64,
    total: u64,
    faces: u64,
    clusters: usize,
    candidates: usize,
    failures: u64,
}

impl RunOutcome {
    fn message(&self) -> Option<String> {
        (self.failures > 0).then(|| {
            format!(
                "{} file(s) could not be analyzed; {} cluster(s), {} candidate(s)",
                self.failures, self.clusters, self.candidates
            )
        })
    }
}

#[derive(Clone)]
pub(crate) struct FaceAnalysisQueue {
    app: AppHandle,
    library: Arc<Library>,
    cache: Arc<CacheManager>,
    people: Arc<PeopleService>,
    models: Option<FaceModelPaths>,
    /// Loaded on first use: parsing the embedder costs real time and memory, so
    /// an application that never opens the people feature never pays for it.
    analyzer: Arc<Mutex<Option<Arc<FaceAnalyzer>>>>,
    work: Arc<Mutex<FaceWork>>,
    next_job_id: Arc<AtomicU64>,
}

impl FaceAnalysisQueue {
    pub(crate) fn new(
        app: AppHandle,
        library: Arc<Library>,
        cache: Arc<CacheManager>,
        people: Arc<PeopleService>,
        models: Option<FaceModelPaths>,
    ) -> Self {
        Self {
            app,
            library,
            cache,
            people,
            models,
            analyzer: Arc::new(Mutex::new(None)),
            work: Arc::new(Mutex::new(FaceWork::default())),
            next_job_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub(crate) fn is_available(&self) -> bool {
        self.models.is_some()
    }

    pub(crate) fn progress(&self) -> Option<FaceAnalysisProgress> {
        self.work
            .lock()
            .expect("face queue lock poisoned")
            .progress
            .clone()
    }

    pub(crate) fn is_running(&self) -> bool {
        self.work
            .lock()
            .expect("face queue lock poisoned")
            .active
            .is_some()
    }

    /// Starts a run, or queues one behind the active run. Returns the job id.
    pub(crate) fn start(&self, request: FaceAnalysisRequest) -> Result<String, String> {
        if self.models.is_none() {
            return Err(
                "face models are not installed; run `pnpm faces:prepare` or install the model pack"
                    .to_owned(),
            );
        }
        {
            let mut work = self.work.lock().expect("face queue lock poisoned");
            if let Some(active) = work.active.as_ref() {
                let job_id = active.job_id.clone();
                work.queued = Some(request);
                return Ok(job_id);
            }
        }
        let job_id = format!(
            "face-job-{}",
            self.next_job_id.fetch_add(1, Ordering::Relaxed)
        );
        let cancellation = CancellationToken::default();
        {
            let mut work = self.work.lock().expect("face queue lock poisoned");
            work.active = Some(ActiveRun {
                job_id: job_id.clone(),
                cancellation: cancellation.clone(),
            });
        }

        let queue = self.clone();
        let run_job_id = job_id.clone();
        std::thread::Builder::new()
            .name("oxy-face-analysis".into())
            .spawn(move || {
                queue.run(&run_job_id, request, cancellation);
                queue.finish(&run_job_id);
            })
            .map_err(|error| format!("failed to start face analysis: {error}"))?;
        Ok(job_id)
    }

    pub(crate) fn cancel(&self, job_id: &str) -> bool {
        let work = self.work.lock().expect("face queue lock poisoned");
        let Some(active) = work.active.as_ref() else {
            return false;
        };
        if active.job_id != job_id {
            return false;
        }
        active.cancellation.cancel();
        true
    }

    fn finish(&self, job_id: &str) {
        let next = {
            let mut work = self.work.lock().expect("face queue lock poisoned");
            if work
                .active
                .as_ref()
                .is_some_and(|active| active.job_id == job_id)
            {
                work.active = None;
            }
            work.queued.take()
        };
        if let Some(request) = next {
            let _ = self.start(request);
        }
    }

    fn analyzer(&self) -> Result<Arc<FaceAnalyzer>, String> {
        let mut cached = self.analyzer.lock().expect("face analyzer lock poisoned");
        if let Some(analyzer) = cached.as_ref() {
            return Ok(analyzer.clone());
        }
        let models = self
            .models
            .as_ref()
            .ok_or_else(|| "face models are not installed".to_owned())?;
        let settings = self
            .library
            .face_analyzer_settings()
            .map_err(|error| error.to_string())?;
        let analyzer =
            Arc::new(FaceAnalyzer::load(models, settings).map_err(|error| error.to_string())?);
        *cached = Some(analyzer.clone());
        Ok(analyzer)
    }

    fn run(&self, job_id: &str, request: FaceAnalysisRequest, cancellation: CancellationToken) {
        let outcome = self.run_inner(job_id, &request, &cancellation);
        let (stage, processed, total, faces, pending, failures, message) = match outcome {
            Ok(outcome) if cancellation.is_cancelled() => (
                FaceAnalysisStage::Idle,
                outcome.processed,
                outcome.total,
                outcome.faces,
                0,
                outcome.failures,
                Some("cancelled".to_owned()),
            ),
            Ok(outcome) => {
                let pending = self
                    .library
                    .face_library_stats()
                    .map_or(0, |stats| stats.pending_reviews);
                (
                    FaceAnalysisStage::Complete,
                    outcome.processed,
                    outcome.total,
                    outcome.faces,
                    pending,
                    outcome.failures,
                    outcome.message(),
                )
            }
            Err(error) => (FaceAnalysisStage::Failed, 0, 0, 0, 0, 0, Some(error)),
        };
        self.publish(FaceAnalysisProgress {
            job_id: job_id.to_owned(),
            stage,
            processed_assets: processed,
            total_assets: total,
            faces_detected: faces,
            pending_reviews: pending,
            failed_assets: failures,
            message,
        });
        if let Ok(stats) = self.library.face_library_stats() {
            let _ = self.app.emit(FACE_LIBRARY_UPDATED_EVENT, stats);
        }
    }

    fn run_inner(
        &self,
        job_id: &str,
        request: &FaceAnalysisRequest,
        cancellation: &CancellationToken,
    ) -> Result<RunOutcome, String> {
        let analyzer = self.analyzer()?;
        let detector_fingerprint = analyzer.detector_fingerprint();
        let embedder_fingerprint = analyzer.embedder_fingerprint().to_owned();

        // Explicit selection wins, then the browsed directory, then the whole
        // library. The directory scope is what makes "analyze this folder"
        // cover a folder larger than the pages the grid has loaded.
        let targets = match (
            &request.paths.is_empty(),
            &request.root_path,
            &request.directory,
        ) {
            (false, _, _) => self.library.face_analysis_targets_for_paths(
                &request.paths,
                &detector_fingerprint,
                request.force,
            ),
            (true, Some(root_path), Some(directory)) => {
                self.library.face_analysis_targets_in_directory(
                    root_path,
                    directory,
                    &detector_fingerprint,
                    request.force,
                    usize::MAX,
                )
            }
            _ => {
                self.library
                    .face_analysis_targets(&detector_fingerprint, request.force, usize::MAX)
            }
        }
        .map_err(|error| error.to_string())?;

        let total = targets.len() as u64;
        let mut processed = 0u64;
        let mut faces = 0u64;
        let mut failures = 0u64;
        let mut last_publish = std::time::Instant::now();

        for target in &targets {
            if cancellation.is_cancelled() {
                return Ok(RunOutcome {
                    processed,
                    total,
                    faces,
                    clusters: 0,
                    candidates: 0,
                    failures,
                });
            }
            // Analysis is background work: never hold the decode gate ahead of
            // an interactive request.
            self.library.foreground.wait_for_background();
            match self.analyze_one(&analyzer, target, cancellation) {
                Ok(count) => faces += count,
                Err(error) => {
                    // One unreadable or unsupported file must not stop the run.
                    failures += 1;
                    eprintln!("face analysis skipped {}: {error}", target.path.display());
                }
            }
            processed += 1;
            if last_publish.elapsed() >= PROGRESS_INTERVAL || processed == total {
                last_publish = std::time::Instant::now();
                let pending = self
                    .library
                    .face_library_stats()
                    .map_or(0, |stats| stats.pending_reviews);
                self.publish(FaceAnalysisProgress {
                    job_id: job_id.to_owned(),
                    stage: FaceAnalysisStage::Detecting,
                    processed_assets: processed,
                    total_assets: total,
                    faces_detected: faces,
                    pending_reviews: pending,
                    failed_assets: failures,
                    message: None,
                });
            }
        }

        if cancellation.is_cancelled() {
            return Ok(RunOutcome {
                processed,
                total,
                faces,
                clusters: 0,
                candidates: 0,
                failures,
            });
        }

        self.publish(FaceAnalysisProgress {
            job_id: job_id.to_owned(),
            stage: FaceAnalysisStage::Clustering,
            processed_assets: processed,
            total_assets: total,
            faces_detected: faces,
            pending_reviews: 0,
            failed_assets: failures,
            message: None,
        });
        let (clusters, candidates) = self.refresh_derived(&embedder_fingerprint, cancellation)?;

        // Analysis re-bound decisions to its own observation ids. Copy those
        // bindings into the durable file so a restart does not show every
        // confirmation as unbound until the next run.
        if let Err(error) = self.people.sync_bindings() {
            eprintln!("failed to persist face decision bindings: {error}");
        }

        Ok(RunOutcome {
            processed,
            total,
            faces,
            clusters,
            candidates,
            failures,
        })
    }

    /// Analyzes one asset and records the result under a verified revision.
    fn analyze_one(
        &self,
        analyzer: &FaceAnalyzer,
        target: &FaceAnalysisTarget,
        cancellation: &CancellationToken,
    ) -> Result<u64, String> {
        // Read the revision before decoding, so the pixels and the revision
        // describe the same bytes; the analyzer re-checks it before returning.
        let revision = oxy_domain::face_source_revision_for_path(&target.path)
            .map_err(|error| error.to_string())?;
        let asset_id = oxy_fs::stable_asset_id(&target.path);

        // Face coordinates and embeddings must come from the final Full
        // artifact, never an interim Preview returned while Full develops.
        let preview = oxy_media::preview_for_app_upgrade(
            &target.path,
            &self.cache.preview_dir(),
            RenderLevel::Full,
            PreviewPriority::Preload,
            target.kind,
            cancellation,
        )
        .map_err(|error| error.to_string())?
        .result;
        if cancellation.is_cancelled() {
            return Ok(0);
        }
        let mut pixels =
            oxy_media::decode_rgb_pixels(&preview.path).map_err(|error| error.to_string())?;
        // Full resolves to the encoded source for these raster formats. Other
        // full artifacts have already applied their display orientation.
        if oxy_domain::full_resolution_is_the_source(target.kind) {
            let orientation = preview
                .image_facts
                .as_ref()
                .map_or(1, |facts| facts.exif_orientation);
            pixels = oxy_media::apply_exif_orientation(pixels, orientation)
                .map_err(|error| error.to_string())?;
        }
        let image = RgbImage::new(pixels.width, pixels.height, pixels.data)
            .map_err(|error| error.to_string())?;

        let Some(analyzed) = analyzer
            .analyze_verified(&asset_id, &target.path, &revision, &image)
            .map_err(|error| error.to_string())?
        else {
            // The file was replaced while it was being analyzed. Leaving the
            // checkpoint untouched makes the next run visit it again.
            return Err("source changed during analysis; result discarded".to_owned());
        };

        let faces: Vec<oxy_library::StoredFace> = analyzed
            .into_iter()
            .map(|face| oxy_library::StoredFace {
                observation: face.observation,
                embedding: face.embedding,
            })
            .collect();
        let pixels = oxy_media::RgbPixels {
            width: image.width(),
            height: image.height(),
            data: image.into_data(),
        };
        let crops: Vec<oxy_library::StoredFaceCrop> = faces
            .iter()
            .filter_map(|face| {
                let observation = &face.observation;
                match oxy_media::face_crop_jpeg(
                    &pixels,
                    observation.bbox,
                    crate::state::face_crops::DEFAULT_CROP_SIZE,
                    crate::state::face_crops::CROP_MARGIN,
                    crate::state::face_crops::CROP_QUALITY,
                ) {
                    Ok((jpeg, _, _)) => Some(oxy_library::StoredFaceCrop {
                        observation_id: observation.observation_id.clone(),
                        size: crate::state::face_crops::DEFAULT_CROP_SIZE,
                        jpeg,
                    }),
                    Err(error) => {
                        eprintln!("scan-time face crop skipped: {error}");
                        None
                    }
                }
            })
            .collect();
        let count = faces.len() as u64;
        self.library
            .replace_asset_faces(
                &target.path,
                &asset_id,
                &revision,
                &analyzer.detector_fingerprint(),
                analyzer.embedder_fingerprint(),
                &faces,
            )
            .map_err(|error| error.to_string())?;
        if let Err(error) = self.library.store_face_crops(&crops) {
            eprintln!("scan-time face crop cache write failed: {error}");
        }
        Ok(count)
    }

    /// Rebuilds clusters and candidates for the current settings.
    ///
    /// Both are cache: each call replaces everything for its fingerprint, so a
    /// changed threshold cannot leave stale suggestions behind.
    fn refresh_derived(
        &self,
        embedder_fingerprint: &str,
        cancellation: &CancellationToken,
    ) -> Result<(usize, usize), String> {
        let settings = self
            .library
            .face_analyzer_settings()
            .map_err(|error| error.to_string())?;

        let embeddings = self
            .library
            .undecided_face_embeddings(embedder_fingerprint, CLUSTER_LIMIT)
            .map_err(|error| error.to_string())?;
        let inputs: Vec<ClusterInput> = embeddings
            .iter()
            .map(|(observation_id, embedding)| ClusterInput {
                observation_id: observation_id.clone(),
                embedding: embedding.clone(),
            })
            .collect();
        let clusters: Vec<FaceCluster> = cluster_embeddings(&inputs, settings.cluster_threshold, 2)
            .map_err(|error| error.to_string())?;
        let clustering_fingerprint = format!(
            "{embedder_fingerprint}/cluster{:.3}",
            settings.cluster_threshold
        );
        self.library
            .replace_face_clusters(&clustering_fingerprint, &clusters)
            .map_err(|error| error.to_string())?;

        if cancellation.is_cancelled() {
            return Ok((clusters.len(), 0));
        }

        let confirmed = self
            .library
            .confirmed_face_embeddings(embedder_fingerprint)
            .map_err(|error| error.to_string())?;
        let persons = self.library.persons().map_err(|error| error.to_string())?;
        let galleries: Vec<PersonGallery> = persons
            .iter()
            .map(|person| {
                let examples: Vec<Vec<f32>> = confirmed
                    .iter()
                    .filter(|(person_id, _)| person_id == &person.person_id)
                    .map(|(_, embedding)| embedding.clone())
                    .collect();
                PersonGallery::new(&person.person_id, &person.display_name, examples)
            })
            .filter(|gallery| !gallery.is_empty())
            .collect();

        let threshold = settings.effective_match_threshold();
        let matcher_fingerprint = format!("{embedder_fingerprint}/match{threshold:.3}");
        let mut candidates = Vec::new();
        if !galleries.is_empty() {
            let unresolved = self
                .library
                .unresolved_face_embeddings(embedder_fingerprint, CANDIDATE_LIMIT)
                .map_err(|error| error.to_string())?;
            for (observation_id, embedding) in unresolved {
                if cancellation.is_cancelled() {
                    return Ok((clusters.len(), candidates.len()));
                }
                let Some(matched) = match_person(&embedding, &galleries) else {
                    continue;
                };
                if !matched.is_candidate(threshold) {
                    continue;
                }
                candidates.push(FaceCandidate {
                    observation_id,
                    person_id: matched.person_id,
                    person_name: matched.display_name,
                    similarity: matched.similarity,
                    matcher_fingerprint: matcher_fingerprint.clone(),
                });
            }
        }
        self.library
            .replace_face_candidates(&matcher_fingerprint, &candidates)
            .map_err(|error| error.to_string())?;
        Ok((clusters.len(), candidates.len()))
    }

    fn publish(&self, progress: FaceAnalysisProgress) {
        self.work.lock().expect("face queue lock poisoned").progress = Some(progress.clone());
        let _ = self.app.emit(FACE_ANALYSIS_PROGRESS_EVENT, progress);
    }

    /// Recomputes clusters and candidates without decoding anything. Settings
    /// changes use this: matcher and cluster thresholds never re-run detection.
    pub(crate) fn refresh_without_detection(&self) -> Result<(), String> {
        if self.models.is_none() {
            return Err("face models are not installed".to_owned());
        }
        self.library.foreground.wait_for_background();
        let analyzer = self.analyzer()?;
        let embedder_fingerprint = analyzer.embedder_fingerprint().to_owned();
        self.refresh_derived(&embedder_fingerprint, &CancellationToken::default())?;
        if let Ok(stats) = self.library.face_library_stats() {
            let _ = self.app.emit(FACE_LIBRARY_UPDATED_EVENT, stats);
        }
        Ok(())
    }

    /// Drops the loaded analyzer so the next run re-reads model files and
    /// settings.
    pub(crate) fn invalidate_analyzer(&self) {
        *self.analyzer.lock().expect("face analyzer lock poisoned") = None;
    }
}

/// Convenience for commands that only need the aggregate counters.
pub(crate) fn library_stats(library: &Library) -> Result<FaceLibraryStats, String> {
    library
        .face_library_stats()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_directory_needs_both_files() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            model_paths_in(directory.path()).is_none(),
            "an empty directory is not a model pack"
        );
        std::fs::write(directory.path().join(DETECTOR_MODEL_FILE), b"onnx").unwrap();
        assert!(
            model_paths_in(directory.path()).is_none(),
            "a detector alone cannot analyze faces"
        );
        std::fs::write(directory.path().join(EMBEDDER_MODEL_FILE), b"onnx").unwrap();
        let paths = model_paths_in(directory.path()).expect("both files present");
        assert_eq!(paths.detector.file_name().unwrap(), DETECTOR_MODEL_FILE);
        assert_eq!(paths.embedder.file_name().unwrap(), EMBEDDER_MODEL_FILE);
        assert_eq!(paths.detector_input_size, DETECTOR_INPUT_SIZE);
    }

    #[test]
    fn the_bundled_layout_is_the_one_the_packager_produces() {
        // `prepare.mjs` stages both files plus their licenses into the resource
        // directory, and the runtime looks for them under `face-models`.
        let staged = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/face-models");
        if !staged.is_dir() {
            // A checkout without `pnpm faces:prepare` is a supported state.
            return;
        }
        let paths = model_paths_in(&staged).expect("staged pack has both models");
        assert!(paths.detector.is_file() && paths.embedder.is_file());
        assert!(
            staged.join("LICENSE-yunet").is_file() && staged.join("LICENSE-sface").is_file(),
            "upstream license texts must ship beside the models"
        );
    }
}
