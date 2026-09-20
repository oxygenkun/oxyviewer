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
use std::sync::{Arc, Mutex};

use oxy_analyzer_host::{AnalyzeInput, Analyzer, ProcessAnalyzer};
use oxy_domain::{
    FaceAnalysisProgress, FaceAnalysisRequest, FaceAnalysisStage, FaceCandidate, FaceCluster,
    FaceLibraryStats, PreviewPriority, RenderLevel,
};
use oxy_faces::{
    ClusterInput, FaceModelPaths, PersonGallery, RgbImage, cluster_embeddings_cancellable,
    match_person_cancellable,
};
use oxy_library::{FaceAnalysisTarget, Library};
use oxy_runtime::{CancellationToken, JobRegistry};
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
/// Model filenames and detector size come from the shared managed manifest.
const FACE_QUALITY_POLICY_VERSION: &str = "laplacian-v1";
const FACE_CLUSTER_POLICY_VERSION: &str = "reciprocal-average-v1";

pub(crate) fn managed_model_paths_in(directory: &Path) -> Option<FaceModelPaths> {
    oxy_faces::managed_model_paths(directory)
}

/// Resolves the pinned model files. A missing directory is a supported state.
pub(crate) fn resolve_model_paths(app: &AppHandle) -> Option<FaceModelPaths> {
    if let Some(directory) = std::env::var_os("OXY_FACE_MODEL_DIR") {
        return managed_model_paths_in(&PathBuf::from(directory));
    }
    if let Ok(data) = app.path().app_data_dir() {
        if let Some(paths) = crate::providers::face_models::installed_pair(&data) {
            return Some(paths);
        }
    }
    None
}

#[derive(Default)]
struct FaceWork {
    active: Option<ActiveRun>,
    progress: Option<FaceAnalysisProgress>,
    /// Requests arriving while a run is active are coalesced into one follow-up.
    queued: Option<FaceWorkRequest>,
}

#[derive(Clone)]
enum FaceWorkRequest {
    Analyze(FaceAnalysisRequest),
    Refresh,
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
    models: Arc<Mutex<Option<FaceModelPaths>>>,
    /// Loaded on first use: parsing the embedder costs real time and memory, so
    /// an application that never opens the people feature never pays for it.
    analyzer: Arc<Mutex<Option<Arc<dyn Analyzer>>>>,
    work: Arc<Mutex<FaceWork>>,
    jobs: Arc<JobRegistry>,
}

impl FaceAnalysisQueue {
    pub(crate) fn app_data_dir(&self) -> Result<PathBuf, String> {
        self.app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())
    }

    pub(crate) fn new(
        app: AppHandle,
        library: Arc<Library>,
        cache: Arc<CacheManager>,
        people: Arc<PeopleService>,
        models: Option<FaceModelPaths>,
        jobs: Arc<JobRegistry>,
    ) -> Self {
        Self {
            app,
            library,
            cache,
            people,
            models: Arc::new(Mutex::new(models)),
            analyzer: Arc::new(Mutex::new(None)),
            work: Arc::new(Mutex::new(FaceWork::default())),
            jobs,
        }
    }

    pub(crate) fn is_available(&self) -> bool {
        self.models
            .lock()
            .expect("face models lock poisoned")
            .is_some()
    }

    pub(crate) fn install_models(&self, models: FaceModelPaths) -> Result<(), String> {
        if self.is_running() {
            return Err("wait for face analysis to finish before changing models".into());
        }
        *self.models.lock().expect("face models lock poisoned") = Some(models);
        *self.analyzer.lock().expect("face analyzer lock poisoned") = None;
        Ok(())
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
    pub(crate) fn start(&self, mut request: FaceAnalysisRequest) -> Result<String, String> {
        let roots = self.library.roots().map_err(|error| error.to_string())?;
        if request.root_path.is_some() != request.directory.is_some() {
            return Err("directory analysis requires an explicit registered root".into());
        }
        if let Some(root) = &request.root_path {
            if !roots.contains(root) {
                return Err("unregistered analysis root".into());
            }
            if let Some(directory) = &mut request.directory {
                *directory = oxy_fs::FsCatalog::authorize_path(root, directory)
                    .map_err(|_| "directory outside authorized root")?;
            }
        }
        for path in &mut request.paths {
            let authorized = roots
                .iter()
                .filter(|root| {
                    request
                        .root_path
                        .as_ref()
                        .is_none_or(|selected| selected == *root)
                })
                .find_map(|root| oxy_fs::FsCatalog::authorize_path(root, path).ok());
            *path = authorized.ok_or("asset outside authorized roots")?;
        }
        request.paths.sort();
        request.paths.dedup();
        self.start_work(FaceWorkRequest::Analyze(request))
    }

    fn start_work(&self, request: FaceWorkRequest) -> Result<String, String> {
        if !self.is_available() {
            return Err(
                "face models are not installed; download them from the People workbench".to_owned(),
            );
        }
        let mut work = self.work.lock().expect("face queue lock poisoned");
        if self.jobs.is_shutting_down() {
            return Err("application is shutting down".into());
        }
        if let Some(active) = work.active.as_ref() {
            let job_id = active.job_id.clone();
            // A refresh must not replace a queued explicit analysis request.
            if !matches!(
                (&work.queued, &request),
                (Some(FaceWorkRequest::Analyze(_)), FaceWorkRequest::Refresh)
            ) {
                work.queued = Some(request);
            }
            return Ok(job_id);
        }
        let ticket = self.jobs.register(oxy_domain::JobPriority::LibraryIndex);
        let job_id = ticket.id.clone();
        let cancellation = ticket.cancellation_token();
        work.active = Some(ActiveRun {
            job_id: job_id.clone(),
            cancellation: cancellation.clone(),
        });
        drop(work);

        let queue = self.clone();
        let run_job_id = job_id.clone();
        let worker = std::thread::Builder::new()
            .name("oxy-face-analysis".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    queue.run(&run_job_id, request, cancellation);
                }));
                if result.is_err() {
                    queue.publish(FaceAnalysisProgress {
                        job_id: run_job_id.clone(),
                        stage: FaceAnalysisStage::Failed,
                        processed_assets: 0,
                        total_assets: 0,
                        faces_detected: 0,
                        pending_reviews: 0,
                        failed_assets: 0,
                        message: Some("face worker panicked".into()),
                    });
                }
                queue.finish(&run_job_id);
            })
            .map_err(|error| {
                self.finish(&job_id);
                format!("failed to start face analysis: {error}")
            })?;
        self.jobs.track_worker(job_id.clone(), worker);
        Ok(job_id)
    }

    pub(crate) fn clear_cache(&self) -> Result<(), String> {
        {
            let mut work = self.work.lock().expect("face queue lock poisoned");
            work.queued = None;
            if let Some(active) = &work.active {
                self.jobs.cancel(&active.job_id);
            }
        }
        self.library
            .clear_face_cache()
            .map_err(|error| error.to_string())?;
        self.people.sync_bindings()?;
        if let Ok(stats) = self.library.face_library_stats() {
            let _ = self.app.emit(FACE_LIBRARY_UPDATED_EVENT, stats);
        }
        Ok(())
    }

    pub(crate) fn cancel(&self, job_id: &str) -> bool {
        let mut work = self.work.lock().expect("face queue lock poisoned");
        let Some(active) = work.active.as_ref() else {
            return false;
        };
        if active.job_id != job_id {
            return false;
        }
        work.queued = None;
        self.jobs.cancel(job_id)
    }

    fn finish(&self, job_id: &str) {
        self.jobs.finish(job_id);
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
            // A coalesced follow-up starts immediately, so keep the parsed
            // models across that boundary. The final job still releases them
            // below; SCRFD/AdaFace never become an unbounded idle cache.
            if self.start_work(request).is_err() {
                *self.analyzer.lock().expect("face analyzer lock poisoned") = None;
            }
        } else {
            *self.analyzer.lock().expect("face analyzer lock poisoned") = None;
        }
    }

    fn analyzer(&self, cancellation: &CancellationToken) -> Result<Arc<dyn Analyzer>, String> {
        if let Some(analyzer) = self
            .analyzer
            .lock()
            .expect("face analyzer lock poisoned")
            .as_ref()
        {
            return Ok(analyzer.clone());
        }
        let models = self
            .models
            .lock()
            .expect("face models lock poisoned")
            .clone()
            .ok_or_else(|| "face models are not installed".to_owned())?;
        let settings = self
            .library
            .face_analyzer_settings()
            .map_err(|error| error.to_string())?;
        let bundled = self
            .app
            .path()
            .resource_dir()
            .map_err(|error| error.to_string())?
            .join("analyzers/faces");
        let development =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/native/analyzers/faces");
        let pack = if bundled.join("manifest.json").is_file() {
            bundled
        } else if cfg!(debug_assertions) {
            development
        } else {
            bundled
        };
        let model_root = models.detector.parent().ok_or("missing model directory")?;
        let analyzer: Arc<dyn Analyzer> = Arc::new(
            ProcessAnalyzer::load(&pack, model_root, settings, cancellation)
                .map_err(|error| error.to_string())?,
        );
        if cancellation.is_cancelled() {
            return Err("cancelled".into());
        }
        *self.analyzer.lock().expect("face analyzer lock poisoned") = Some(analyzer.clone());
        Ok(analyzer)
    }

    fn run(&self, job_id: &str, request: FaceWorkRequest, cancellation: CancellationToken) {
        let outcome = match request {
            FaceWorkRequest::Analyze(request) => self.run_inner(job_id, &request, &cancellation),
            FaceWorkRequest::Refresh => self.run_refresh(&cancellation),
        };
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
            Err(_) if cancellation.is_cancelled() => (
                FaceAnalysisStage::Idle,
                0,
                0,
                0,
                0,
                0,
                Some("cancelled".into()),
            ),
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
        let analyzer = self.analyzer(cancellation)?;
        let embedder_fingerprint = analyzer
            .descriptor()
            .feature_fingerprint
            .as_str()
            .to_owned();
        let detector_fingerprint = format!(
            "{}/quality-{FACE_QUALITY_POLICY_VERSION}",
            analyzer.descriptor().analysis_fingerprint
        );

        let run_key = format!(
            "{}:{}",
            detector_fingerprint,
            serde_json::to_string(request).map_err(|error| error.to_string())?
        );
        let mut checkpoint = self
            .library
            .resume_face_run(&run_key)
            .map_err(|error| error.to_string())?;
        let mut cursor = checkpoint.cursor.clone();
        let mut total = 0u64;
        let mut processed = 0u64;
        let mut faces = 0u64;
        let mut failures = 0u64;
        let mut last_publish = std::time::Instant::now();

        loop {
            if cancellation.is_cancelled() {
                break;
            }
            let latest = self
                .library
                .resume_face_run(&run_key)
                .map_err(|error| error.to_string())?;
            if latest.generation != checkpoint.generation {
                checkpoint = latest;
                cursor = checkpoint.cursor.clone();
            }
            let targets = self
                .library
                .face_analysis_page(request, cursor.as_deref())
                .map_err(|error| error.to_string())?;
            if targets.is_empty() {
                if self
                    .library
                    .finish_face_run(&run_key, checkpoint.generation)
                    .map_err(|error| error.to_string())?
                {
                    break;
                }
                continue;
            }
            total += targets.len() as u64;
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
                if !self
                    .library
                    .foreground
                    .wait_for_background_cancellable(cancellation)
                {
                    break;
                }
                match self.analyze_one(analyzer.as_ref(), target, request.force, cancellation) {
                    Ok(count) => faces += count,
                    Err(error) => {
                        // One unreadable or unsupported file must not stop the run.
                        failures += 1;
                        eprintln!(
                            "face analysis failed for asset {}: {error}",
                            oxy_fs::stable_asset_id(&target.path)
                        );
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
                break;
            }
            cursor = targets.last().map(|target| target.path.clone());
            if let Some(cursor) = &cursor {
                self.library
                    .checkpoint_face_run(&run_key, checkpoint.generation, cursor)
                    .map_err(|error| error.to_string())?;
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
        analyzer: &dyn Analyzer,
        target: &FaceAnalysisTarget,
        force: bool,
        cancellation: &CancellationToken,
    ) -> Result<u64, String> {
        // Read the revision before decoding, so the pixels and the revision
        // describe the same bytes; the host re-checks it before committing.
        self.authorize_target(&target.path)?;
        let revision = oxy_fs::observe_source_revision(&target.path)
            .map_err(|error| error.to_string())?
            .revision_id;
        let detector_fingerprint = format!(
            "{}/quality-{FACE_QUALITY_POLICY_VERSION}",
            analyzer.descriptor().analysis_fingerprint
        );
        if !force
            && self
                .library
                .face_models_are_current(
                    &target.path,
                    &revision,
                    &detector_fingerprint,
                    &analyzer.descriptor().feature_fingerprint,
                )
                .map_err(|error| error.to_string())?
        {
            return Ok(0);
        }
        let asset_id = oxy_fs::stable_asset_id(&target.path);

        let valid_at = self
            .library
            .begin_face_analysis(&target.path, &revision)
            .map_err(|error| error.to_string())?;
        let outcome = (|| {
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

            self.verify_source(&target.path, &revision)?;
            let analyzed = analyzer
                .analyze(
                    &AnalyzeInput {
                        asset_id: &asset_id,
                        source_revision: &revision,
                        width: image.width(),
                        height: image.height(),
                        pixels: image.data(),
                    },
                    cancellation,
                )
                .map_err(|error| error.to_string())?;
            let faces: Vec<oxy_library::StoredFace> = analyzed
                .regions
                .into_iter()
                .zip(analyzed.features)
                .map(|(region, embedding)| {
                    let (face_pixels, clarity) = face_quality(&image, region.rect);
                    oxy_library::StoredFace {
                        observation: oxy_domain::FaceObservation {
                            observation_id: region.id,
                            asset_id: asset_id.clone(),
                            asset_path: target.path.clone(),
                            source_revision: revision.clone(),
                            local_index: region.index,
                            bbox: region.rect,
                            landmarks: region.landmarks,
                            detection_score: region.score,
                            detector_fingerprint: detector_fingerprint.clone(),
                        },
                        embedding,
                        face_pixels,
                        clarity,
                    }
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
            if cancellation.is_cancelled() {
                return Err("cancelled".into());
            }
            self.verify_source(&target.path, &revision)?;
            self.library
                .accept_face_result(oxy_library::FaceResultWrite {
                    asset_path: &target.path,
                    asset_id: &asset_id,
                    source_revision: &revision,
                    detector_fingerprint: &detector_fingerprint,
                    embedder_fingerprint: analyzer.descriptor().feature_fingerprint.as_str(),
                    faces: &faces,
                    crops: &crops,
                    valid_at: Some(valid_at),
                    cancellation: Some(cancellation),
                })
                .map_err(|error| error.to_string())?;
            Ok(count)
        })();
        if outcome.is_err() || cancellation.is_cancelled() {
            self.library
                .fail_face_analysis(
                    &target.path,
                    valid_at,
                    if cancellation.is_cancelled() {
                        "cancelled"
                    } else {
                        "analysisFailed"
                    },
                )
                .map_err(|error| error.to_string())?;
        }
        outcome
    }

    fn authorize_target(&self, path: &Path) -> Result<(), String> {
        let roots = self.library.roots().map_err(|error| error.to_string())?;
        if roots.iter().any(|root| {
            oxy_fs::FsCatalog::authorize_path(root, path).is_ok_and(|canonical| canonical == path)
        }) {
            Ok(())
        } else {
            Err("asset is no longer inside an authorized root".into())
        }
    }

    fn verify_source(&self, path: &Path, expected: &str) -> Result<(), String> {
        self.authorize_target(path)?;
        let current = oxy_fs::observe_source_revision(path).map_err(|error| error.to_string())?;
        if current.revision_id == expected {
            Ok(())
        } else {
            Err("source changed; result discarded".into())
        }
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
            .undecided_face_cluster_embeddings(embedder_fingerprint, CLUSTER_LIMIT + 1)
            .map_err(|error| error.to_string())?;
        let inputs: Vec<ClusterInput> = embeddings
            .iter()
            .map(|(observation_id, asset_id, embedding)| ClusterInput {
                observation_id: observation_id.clone(),
                asset_id: Some(asset_id.clone()),
                embedding: embedding.clone(),
            })
            .collect();
        let clusters: Vec<FaceCluster> =
            cluster_embeddings_cancellable(&inputs, settings.cluster_threshold, 2, || {
                cancellation.is_cancelled()
            })
            .map_err(|error| error.to_string())?;
        let clustering_fingerprint = format!(
            "{embedder_fingerprint}/{FACE_CLUSTER_POLICY_VERSION}/cluster{:08x}",
            settings.cluster_threshold.to_bits()
        );
        if cancellation.is_cancelled() {
            return Err("cancelled".into());
        }

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
        let matcher_fingerprint =
            format!("{embedder_fingerprint}/match{:08x}", threshold.to_bits());
        let mut candidates = Vec::new();
        if !galleries.is_empty() {
            let unresolved = self
                .library
                .unresolved_face_embeddings(embedder_fingerprint, CANDIDATE_LIMIT + 1)
                .map_err(|error| error.to_string())?;
            if unresolved.len() > CANDIDATE_LIMIT {
                return Err(format!(
                    "matching limit exceeded: more than {CANDIDATE_LIMIT} unresolved faces"
                ));
            }
            for (observation_id, embedding) in unresolved {
                if cancellation.is_cancelled() {
                    return Ok((clusters.len(), candidates.len()));
                }
                let Some(matched) = match_person_cancellable(&embedding, &galleries, || {
                    cancellation.is_cancelled()
                })
                .map_err(|error| error.to_string())?
                else {
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
            .replace_face_derived(
                &settings,
                &clustering_fingerprint,
                &clusters,
                &matcher_fingerprint,
                &candidates,
                cancellation,
            )
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
        self.start_work(FaceWorkRequest::Refresh).map(|_| ())
    }

    fn run_refresh(&self, cancellation: &CancellationToken) -> Result<RunOutcome, String> {
        if !self
            .library
            .foreground
            .wait_for_background_cancellable(cancellation)
        {
            return Err("cancelled".into());
        }
        let analyzer = self.analyzer(cancellation)?;
        let (clusters, candidates) = self.refresh_derived(
            analyzer.descriptor().feature_fingerprint.as_str(),
            cancellation,
        )?;
        Ok(RunOutcome {
            processed: 0,
            total: 0,
            faces: 0,
            clusters,
            candidates,
            failures: 0,
        })
    }

    /// Drops the loaded analyzer so the next run re-reads model files and
    /// settings.
    pub(crate) fn invalidate_analyzer(&self, detection_changed: bool) -> Result<(), String> {
        if let Some(active) = self
            .work
            .lock()
            .expect("face queue lock poisoned")
            .active
            .as_ref()
        {
            active.cancellation.cancel();
        }
        if detection_changed {
            self.library
                .invalidate_face_projections()
                .map_err(|error| error.to_string())?;
        }
        *self.analyzer.lock().expect("face analyzer lock poisoned") = None;
        Ok(())
    }
}

/// Computes a cheap, scale-bounded focus heuristic from the detected face.
/// Sampling at most roughly 96 points per axis keeps this linear in a small
/// review crop rather than in the source resolution.
fn face_quality(image: &RgbImage, rect: oxy_domain::NormalizedRect) -> (u32, f32) {
    let Some(rect) = rect.clamp_unit() else {
        return (0, 0.0);
    };
    let x0 = (rect.x * image.width() as f32).floor() as u32;
    let y0 = (rect.y * image.height() as f32).floor() as u32;
    let width = (rect.width * image.width() as f32).round() as u32;
    let height = (rect.height * image.height() as f32).round() as u32;
    let face_pixels = width.min(height);
    if face_pixels < 3 {
        return (face_pixels, 0.0);
    }
    let x1 = x0.saturating_add(width).min(image.width());
    let y1 = y0.saturating_add(height).min(image.height());
    let step = (face_pixels / 96).max(1);
    if x1 <= x0 + step * 2 || y1 <= y0 + step * 2 {
        return (face_pixels, 0.0);
    }

    let luminance = |x: u32, y: u32| {
        let [red, green, blue] = image.pixel(x, y);
        0.299 * f64::from(red) + 0.587 * f64::from(green) + 0.114 * f64::from(blue)
    };
    let mut samples = 0_u64;
    let mut mean = 0.0_f64;
    let mut squared_delta = 0.0_f64;
    for y in ((y0 + step)..(y1 - step)).step_by(step as usize) {
        for x in ((x0 + step)..(x1 - step)).step_by(step as usize) {
            let laplacian = 4.0 * luminance(x, y)
                - luminance(x - step, y)
                - luminance(x + step, y)
                - luminance(x, y - step)
                - luminance(x, y + step);
            samples += 1;
            let delta = laplacian - mean;
            mean += delta / samples as f64;
            squared_delta += delta * (laplacian - mean);
        }
    }
    let variance = if samples > 1 {
        squared_delta / (samples - 1) as f64
    } else {
        0.0
    };
    let clarity = variance / (variance + 500.0);
    (face_pixels, clarity.clamp(0.0, 1.0) as f32)
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
        let manifest = oxy_faces::managed_model_manifest();
        let detector = manifest
            .models
            .iter()
            .find(|model| model.id == "scrfd-10g-kps")
            .unwrap();
        let embedder = manifest
            .models
            .iter()
            .find(|model| model.id == "adaface-ir101")
            .unwrap();
        assert!(
            managed_model_paths_in(directory.path()).is_none(),
            "an empty directory is not a model pack"
        );
        std::fs::write(directory.path().join(&detector.file), b"onnx").unwrap();
        assert!(
            managed_model_paths_in(directory.path()).is_none(),
            "a detector alone cannot analyze faces"
        );
        std::fs::write(directory.path().join(&embedder.file), b"onnx").unwrap();
        let paths = managed_model_paths_in(directory.path()).expect("both files present");
        assert_eq!(paths.detector.file_name().unwrap(), detector.file.as_str());
        assert_eq!(paths.embedder.file_name().unwrap(), embedder.file.as_str());
        assert_eq!(paths.detector_input_size, manifest.detector_input_size);
    }

    #[test]
    fn face_quality_separates_flat_and_high_frequency_crops() {
        let flat = RgbImage::new(64, 64, vec![128; 64 * 64 * 3]).unwrap();
        let mut checkerboard = Vec::with_capacity(64 * 64 * 3);
        for y in 0..64 {
            for x in 0..64 {
                let value = if (x + y) % 2 == 0 { 0 } else { 255 };
                checkerboard.extend_from_slice(&[value, value, value]);
            }
        }
        let sharp = RgbImage::new(64, 64, checkerboard).unwrap();
        let rect = oxy_domain::NormalizedRect::new(0.0, 0.0, 1.0, 1.0);
        let (pixels, flat_score) = face_quality(&flat, rect);
        let (_, sharp_score) = face_quality(&sharp, rect);
        assert_eq!(pixels, 64);
        assert_eq!(flat_score, 0.0);
        assert!(sharp_score > 0.9, "checkerboard score was {sharp_score}");
    }
}
