//! Face and person contracts.
//!
//! The split between machine-derived observations and user-authored facts is a
//! core invariant, not an implementation detail:
//!
//! * [`FaceObservation`], [`FaceCandidate`], and [`FaceCluster`] are
//!   rebuildable analyzer output. They may be deleted and recomputed at any
//!   time, and a cluster is never a person.
//! * [`Person`] and [`FaceDecision`] are user data. A new analyzer, detector,
//!   or embedding model must never destroy them.
//!
//! Coordinates are normalized to the display-oriented image (`0.0..=1.0`) so a
//! persisted region stays valid across preview levels and cache rebuilds.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::{AssetId, CustomTagId};

/// Stable, user-data identifier for a person. Never a cluster id, a tag name,
/// or a display name.
pub type PersonId = String;

/// Stable identifier for one machine-detected face region on one asset.
pub type FaceObservationId = String;

/// Revision token version for face analysis.
///
/// The library's scan checkpoint and the analyzer's stale-write guard must agree
/// on this string, so it lives with the contract instead of on either side.
pub const FACE_REVISION_VERSION: &str = "faces-v1";

/// Revision token for the bytes of one asset at analysis time.
///
/// It depends only on what the index already knows (modification time and size),
/// so both sides can compute it without a second stat.
pub fn face_source_revision(modified_at_ms: u64, size_bytes: u64) -> String {
    format!("{modified_at_ms}:{size_bytes}:{FACE_REVISION_VERSION}")
}

/// Revision token read from the file itself.
///
/// A file that changed between the index scan and analysis therefore produces a
/// different token than the checkpoint holds, which is exactly what makes a
/// resumed run re-visit it instead of silently keeping a stale result.
pub fn face_source_revision_for_path(path: &Path) -> std::io::Result<String> {
    let metadata = std::fs::metadata(path)?;
    let modified_at_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_millis() as u64);
    Ok(face_source_revision(modified_at_ms, metadata.len()))
}

/// A rectangle in normalized display-oriented image coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl NormalizedRect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Projects a pixel rectangle onto the display-oriented image.
    pub fn from_pixels(x: f32, y: f32, width: f32, height: f32, image: PixelSize) -> Self {
        if image.width == 0 || image.height == 0 {
            return Self::new(0.0, 0.0, 0.0, 0.0);
        }
        Self {
            x: x / image.width as f32,
            y: y / image.height as f32,
            width: width / image.width as f32,
            height: height / image.height as f32,
        }
    }

    pub fn to_pixels(self, image: PixelSize) -> (f32, f32, f32, f32) {
        (
            self.x * image.width as f32,
            self.y * image.height as f32,
            self.width * image.width as f32,
            self.height * image.height as f32,
        )
    }

    /// True when every value is finite and the rectangle has positive area.
    pub fn is_valid(self) -> bool {
        let values = [self.x, self.y, self.width, self.height];
        values.iter().all(|value| value.is_finite()) && self.width > 0.0 && self.height > 0.0
    }

    /// Clamps the rectangle to `0.0..=1.0`, returning `None` when nothing is
    /// left. Analyzer output is untrusted input and must pass through this
    /// before it is stored.
    pub fn clamp_unit(self) -> Option<Self> {
        if !self.is_valid() {
            return None;
        }
        let x = self.x.max(0.0);
        let y = self.y.max(0.0);
        let right = (self.x + self.width).min(1.0);
        let bottom = (self.y + self.height).min(1.0);
        if right <= x || bottom <= y {
            return None;
        }
        Some(Self::new(x, y, right - x, bottom - y))
    }

    /// Intersection over union in normalized space.
    pub fn iou(self, other: Self) -> f32 {
        if !self.is_valid() || !other.is_valid() {
            return 0.0;
        }
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        let intersection = (right - left).max(0.0) * (bottom - top).max(0.0);
        let union = self.width * self.height + other.width * other.height - intersection;
        if union <= 0.0 {
            0.0
        } else {
            intersection / union
        }
    }

    pub fn center(self) -> NormalizedPoint {
        NormalizedPoint {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }
}

/// A point in normalized display-oriented image coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedPoint {
    pub x: f32,
    pub y: f32,
}

impl NormalizedPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn is_valid(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// Integer pixel dimensions. Shared by the analyzer and the persistence layer
/// so a stored region can always be reconstructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

/// One detected face on one asset. Rebuildable analyzer output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceObservation {
    pub observation_id: FaceObservationId,
    pub asset_id: AssetId,
    pub asset_path: PathBuf,
    /// Asset revision this result was computed from. A result whose revision no
    /// longer matches the authoritative asset revision must be rejected rather
    /// than stored.
    pub source_revision: String,
    /// Index of this face inside its asset, stable only within one detection
    /// fingerprint.
    pub local_index: u32,
    pub bbox: NormalizedRect,
    #[serde(default)]
    pub landmarks: Vec<NormalizedPoint>,
    pub detection_score: f32,
    pub detector_fingerprint: String,
}

impl FaceObservation {
    /// Machine observations are untrusted: reject geometry that a UI could not
    /// draw or that an embedding could not use.
    pub fn is_storable(&self) -> bool {
        !self.observation_id.is_empty()
            && !self.asset_id.is_empty()
            && !self.source_revision.is_empty()
            && self.detection_score.is_finite()
            && self.detection_score >= 0.0
            && self.detection_score <= 1.0
            && self.bbox.clamp_unit().is_some()
            && self.landmarks.iter().all(|point| {
                point.is_valid() && (0.0..=1.0).contains(&point.x) && (0.0..=1.0).contains(&point.y)
            })
    }

    /// The durable region a user decision attaches to. Bounding boxes survive a
    /// detector upgrade in a way detector-local ids do not, so decisions are
    /// bound to geometry and reconciled by overlap.
    pub fn durable_region(&self) -> NormalizedRect {
        self.bbox
    }
}

/// Review state of one observation, derived from user decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FaceReviewState {
    /// No candidate and no decision: this face is still unknown.
    Unknown,
    /// A matcher proposed a person and the user has not answered yet.
    Pending,
    /// The user confirmed the face belongs to a person.
    Confirmed,
    /// The user said the face is not the proposed person.
    Rejected,
    /// The user said the region is not a face at all.
    NotFace,
}

impl FaceReviewState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Pending => "pending",
            Self::Confirmed => "confirmed",
            Self::Rejected => "rejected",
            Self::NotFace => "not_face",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(Self::Unknown),
            "pending" => Some(Self::Pending),
            "confirmed" => Some(Self::Confirmed),
            "rejected" => Some(Self::Rejected),
            "not_face" => Some(Self::NotFace),
            _ => None,
        }
    }
}

/// A matcher proposal that a person is the identity of an observation.
/// Rebuildable analyzer output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceCandidate {
    pub observation_id: FaceObservationId,
    pub person_id: PersonId,
    pub person_name: String,
    /// Cosine similarity between the observation embedding and the person's
    /// gallery. Detection confidence is deliberately not reused here.
    pub similarity: f32,
    pub matcher_fingerprint: String,
}

/// A stable person. User data: renaming never invalidates an assignment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub person_id: PersonId,
    pub display_name: String,
    /// Existing tag used to project this person into search and XMP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_tag_id: Option<CustomTagId>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    /// Number of observations the user confirmed for this person.
    pub face_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_observation_id: Option<FaceObservationId>,
}

/// A suggestion that several unknown observations show one person.
/// Rebuildable analyzer output: never store this as the person's identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceCluster {
    pub cluster_id: String,
    pub observation_ids: Vec<FaceObservationId>,
    pub representative_observation_id: FaceObservationId,
    pub member_count: usize,
    /// Mean pairwise similarity of the cluster members, in `-1.0..=1.0`.
    pub cohesion: f32,
    /// Members that sit far from the rest of the cluster. A review UI shows
    /// them unselected so a mostly-Alice cluster never silently confirms an
    /// outlier into Alice.
    #[serde(default)]
    pub outlier_observation_ids: Vec<FaceObservationId>,
    /// Existing person this cluster most resembles, if one matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_person_id: Option<PersonId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_name: Option<String>,
}

/// One row of the review queue. Combined view of machine output and the
/// user's decision, so the UI never has to join tables itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceReviewItem {
    pub observation_id: FaceObservationId,
    pub asset_id: AssetId,
    pub asset_path: PathBuf,
    pub bbox: NormalizedRect,
    pub detection_score: f32,
    pub state: FaceReviewState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<FaceCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_person_id: Option<PersonId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_person_name: Option<String>,
}

/// What the user asserted about one face region.
///
/// All three variants are durable user data. `RejectPerson` and `NotFace` in
/// particular must be stored rather than implemented as "delete the machine
/// row", otherwise the next analysis run recreates the same review item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "decision",
    rename_all = "camelCase",
    // A tagged enum camel-cases its variant names with `rename_all`, but the
    // fields of a struct variant need `rename_all_fields`. Without it the IPC
    // payload the frontend sends (`personId`) fails to deserialize and every
    // confirm/reject answer is dropped before it reaches the store.
    rename_all_fields = "camelCase"
)]
pub enum FaceDecision {
    ConfirmPerson {
        // Accepted so a `people.json` written before the field names were
        // corrected still loads instead of being quarantined.
        #[serde(alias = "person_id")]
        person_id: PersonId,
    },
    RejectPerson {
        #[serde(alias = "person_id")]
        person_id: PersonId,
    },
    NotFace,
}

impl FaceDecision {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ConfirmPerson { .. } => "confirm_person",
            Self::RejectPerson { .. } => "reject_person",
            Self::NotFace => "not_face",
        }
    }

    pub fn person_id(&self) -> Option<&str> {
        match self {
            Self::ConfirmPerson { person_id } | Self::RejectPerson { person_id } => {
                Some(person_id.as_str())
            }
            Self::NotFace => None,
        }
    }
}

/// Append-only record of a user action, so merge, split, and misclicks can be
/// undone without special-case recovery logic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceDecisionRecord {
    pub event_id: i64,
    pub observation_id: FaceObservationId,
    pub decision: FaceDecision,
    pub region: NormalizedRect,
    pub created_at_ms: u64,
}

/// Durable, cache-independent record of one person.
///
/// This is what a non-SQLite store persists. It deliberately excludes the
/// derived counters on [`Person`] (`face_count`, `cover_observation_id`): those
/// describe the current cache, and a durable file must not depend on rows that
/// the next cache rebuild will replace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonRecord {
    pub person_id: PersonId,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_tag_id: Option<CustomTagId>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl From<&Person> for PersonRecord {
    fn from(person: &Person) -> Self {
        Self {
            person_id: person.person_id.clone(),
            display_name: person.display_name.clone(),
            linked_tag_id: person.linked_tag_id,
            created_at_ms: person.created_at_ms,
            updated_at_ms: person.updated_at_ms,
        }
    }
}

/// Durable, cache-independent record of one user decision.
///
/// `observation_id` is a *current binding*, not the identity of the decision.
/// The decision is bound to `region` on `asset_id`, which is what lets it
/// survive a detector upgrade and a complete cache deletion: after a rebuild,
/// the next analysis re-binds it to the overlapping new observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_id: Option<FaceObservationId>,
    pub asset_id: AssetId,
    pub asset_path: PathBuf,
    pub region: NormalizedRect,
    pub decision: FaceDecision,
    pub created_at_ms: u64,
    /// Similarity the matcher proposed for this face, when it proposed one.
    ///
    /// This is the only place the application learns how a real threshold
    /// behaves on a real library: the score itself does not depend on the
    /// threshold, only whether a candidate was created does. Advisory data —
    /// losing it degrades calibration, never a confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_similarity: Option<f32>,
}

/// The user's answer together with the score the matcher proposed for it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceScoreSample {
    pub similarity: f32,
    pub accepted: bool,
}

/// Measured behaviour of the matcher on this library, for threshold tuning.
///
/// The samples are biased by construction: only faces the *current* threshold
/// promoted to a candidate could be accepted or rejected, so nothing below it
/// appears here. A recommendation is therefore a refinement of the current
/// setting, not an unconditional replacement for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceCalibration {
    pub accepted_scores: Vec<f32>,
    pub rejected_scores: Vec<f32>,
    pub current_threshold: f32,
    /// Suggested threshold, or `None` while there is too little evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_threshold: Option<f32>,
    /// True when every accepted score is above every rejected score, so the two
    /// populations already separate and the exact midpoint is not critical.
    pub separable: bool,
}

/// User-facing matching strictness. Raw cosine thresholds are model-specific,
/// so presets are the primary control and the numeric value is the escape
/// hatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FaceMatchSensitivity {
    Strict,
    #[default]
    Balanced,
    Loose,
    Custom,
}

/// User-controllable analyzer parameters.
///
/// Each field belongs to exactly one recomputation level; changing a matcher
/// threshold must never re-run detection.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceAnalyzerSettings {
    /// Minimum YuNet score. Affects detection output only.
    pub detection_confidence: f32,
    /// IoU threshold used to merge overlapping detections.
    pub nms_threshold: f32,
    /// Upper bound on faces reported per asset.
    pub max_faces_per_asset: u32,
    /// Faces smaller than this many pixels in the analysis image are dropped.
    pub min_face_pixels: u32,
    /// Analyze overlapping tiles in addition to the whole frame.
    ///
    /// A group photo downscaled into one 640x640 detector input loses exactly
    /// the small faces that matter most there. Tiling keeps the effective
    /// resolution up at the cost of extra inference passes, so it is a user
    /// choice rather than a silent default.
    #[serde(default = "default_true")]
    pub detect_small_faces: bool,
    pub match_sensitivity: FaceMatchSensitivity,
    /// Similarity above which a known person becomes a pending candidate.
    pub match_threshold: f32,
    /// Similarity above which a candidate may be accepted without review.
    /// `None` (the default) keeps every candidate pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_accept_threshold: Option<f32>,
    /// Similarity above which two unknown faces may be clustered together.
    pub cluster_threshold: f32,
}

impl Default for FaceAnalyzerSettings {
    fn default() -> Self {
        Self {
            detection_confidence: 0.9,
            nms_threshold: 0.3,
            max_faces_per_asset: 64,
            min_face_pixels: 24,
            detect_small_faces: true,
            match_sensitivity: FaceMatchSensitivity::Balanced,
            match_threshold: FaceMatchSensitivity::Balanced.threshold(),
            auto_accept_threshold: None,
            cluster_threshold: 0.363,
        }
    }
}

impl FaceMatchSensitivity {
    /// SFace cosine presets. These are starting points measured on public
    /// benchmarks, not universal constants; a real library should be calibrated
    /// from its own accepted/rejected score distribution.
    pub fn threshold(self) -> f32 {
        match self {
            Self::Strict => 0.50,
            Self::Balanced | Self::Custom => 0.363,
            Self::Loose => 0.25,
        }
    }
}

impl FaceAnalyzerSettings {
    /// The threshold a matcher should use, resolving the preset against the
    /// stored numeric override.
    pub fn effective_match_threshold(&self) -> f32 {
        if self.match_sensitivity == FaceMatchSensitivity::Custom {
            self.match_threshold
        } else {
            self.match_sensitivity.threshold()
        }
    }

    /// Clamps untrusted settings before they reach the analyzer.
    pub fn sanitized(mut self) -> Self {
        self.detection_confidence = self.detection_confidence.clamp(0.01, 0.99);
        self.nms_threshold = self.nms_threshold.clamp(0.0, 1.0);
        self.max_faces_per_asset = self.max_faces_per_asset.clamp(1, 512);
        self.min_face_pixels = self.min_face_pixels.clamp(8, 512);
        self.match_threshold = self.match_threshold.clamp(-1.0, 1.0);
        self.cluster_threshold = self.cluster_threshold.clamp(0.0, 1.0);
        self.auto_accept_threshold = self
            .auto_accept_threshold
            .map(|value| value.clamp(self.effective_match_threshold(), 1.0));
        self
    }
}

/// Which analyzer levels must be recomputed for a request. The levels are
/// ordered: recomputing an earlier level implies every later one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FaceRecomputeScope {
    /// Only assets with no stored detection for the current fingerprint.
    Missing,
    /// Detection and everything after it.
    Detection,
    /// Alignment and embedding, and everything after them.
    Embedding,
    /// Unknown-face clustering and candidate matching only.
    Clustering,
    /// Candidate matching only.
    Matching,
}

impl FaceRecomputeScope {
    pub fn invalidates_detection(self) -> bool {
        matches!(self, Self::Missing | Self::Detection)
    }

    pub fn invalidates_embedding(self) -> bool {
        matches!(self, Self::Missing | Self::Detection | Self::Embedding)
    }

    pub fn invalidates_clustering(self) -> bool {
        !matches!(self, Self::Matching)
    }
}

/// What one analysis run should visit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceAnalysisRequest {
    /// Explicit assets to visit. Takes precedence over `directory`.
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    /// Browsed directory to analyze, as the folder session holds it.
    ///
    /// This is the useful default for "analyze this folder": it covers every
    /// indexed asset in the directory, not only the pages the grid has loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<PathBuf>,
    /// Ignore the stored scan checkpoint and analyze again.
    #[serde(default)]
    pub force: bool,
}

impl FaceAnalysisRequest {
    /// Every indexed asset in the library.
    pub fn library(force: bool) -> Self {
        Self {
            force,
            ..Self::default()
        }
    }

    /// One browsed directory.
    pub fn directory(root_path: PathBuf, directory: PathBuf, force: bool) -> Self {
        Self {
            root_path: Some(root_path),
            directory: Some(directory),
            force,
            ..Self::default()
        }
    }

    /// An explicit selection.
    pub fn paths(paths: Vec<PathBuf>, force: bool) -> Self {
        Self {
            paths,
            force,
            ..Self::default()
        }
    }
}

/// The asset one face was detected on.
///
/// A crop is keyed by observation, but "show me this photo" needs the asset the
/// observation belongs to. Resolving it on demand keeps the cluster and person
/// contracts free of per-face asset paths they do not otherwise need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceAssetReveal {
    pub observation_id: FaceObservationId,
    pub asset_id: AssetId,
    pub asset_path: PathBuf,
}

/// The directory the main window is browsing, so a workbench run can be scoped
/// to it rather than to the pages the grid happens to have loaded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceWorkbenchScope {
    pub root_path: PathBuf,
    pub directory: PathBuf,
}

/// What the main window tells the face workbench window about itself.
///
/// The workbench is a separate window with its own store, so the browse scope,
/// the loaded selection, and the UI locale cross the window boundary as
/// published state instead of being re-derived (or re-implemented) there.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceWorkbenchContext {
    pub locale: String,
    /// Assets the browser currently shows, for an explicit-selection run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visible_paths: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browse_scope: Option<FaceWorkbenchScope>,
}

/// Stages reported by a face analysis job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FaceAnalysisStage {
    Idle,
    Detecting,
    Embedding,
    Clustering,
    Matching,
    Complete,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceAnalysisProgress {
    pub job_id: String,
    pub stage: FaceAnalysisStage,
    pub processed_assets: u64,
    pub total_assets: u64,
    pub faces_detected: u64,
    pub pending_reviews: u64,
    /// Files this run could not decode or analyze. They are counted as processed
    /// but contributed no faces, so the UI must say so instead of reporting a
    /// clean run whose results are silently short.
    #[serde(default)]
    pub failed_assets: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Counters shown in the people panel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceLibraryStats {
    pub analyzed_assets: u64,
    pub faces_detected: u64,
    pub persons: u64,
    pub pending_reviews: u64,
    pub unknown_faces: u64,
    pub clusters: u64,
}

fn default_true() -> bool {
    true
}

/// Which rows of the review queue to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FaceReviewFilter {
    /// Every stored observation.
    #[default]
    All,
    /// Candidates awaiting a user answer.
    Pending,
    /// Faces with no candidate and no decision.
    Unknown,
    /// Faces the user has not answered at all: `Pending` and `Unknown` together.
    ///
    /// This is the queue a first pass works through. `Pending` alone is empty on
    /// a library with no named people yet, because a matcher proposal needs a
    /// person to propose, so the default view must not be `Pending`.
    Unreviewed,
    /// Faces the user confirmed.
    Confirmed,
    /// Faces the user rejected or marked as not a face.
    Rejected,
}

impl FaceReviewFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Pending => "pending",
            Self::Unknown => "unknown",
            Self::Unreviewed => "unreviewed",
            Self::Confirmed => "confirmed",
            Self::Rejected => "rejected",
        }
    }
}

/// One page of the review queue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceReviewPage {
    pub items: Vec<FaceReviewItem>,
    pub total: usize,
    pub next_cursor: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(bbox: NormalizedRect) -> FaceObservation {
        FaceObservation {
            observation_id: "obs-1".into(),
            asset_id: "asset-1".into(),
            asset_path: PathBuf::from("/photos/a.jpg"),
            source_revision: "100:200:xmp".into(),
            local_index: 0,
            bbox,
            landmarks: vec![NormalizedPoint::new(0.2, 0.2); 5],
            detection_score: 0.95,
            detector_fingerprint: "yunet-2023mar".into(),
        }
    }

    #[test]
    fn normalized_rect_round_trips_pixels() {
        let image = PixelSize {
            width: 400,
            height: 500,
        };
        let rect = NormalizedRect::from_pixels(40.0, 100.0, 80.0, 100.0, image);
        assert!((rect.x - 0.1).abs() < 1e-6);
        assert!((rect.y - 0.2).abs() < 1e-6);
        let (x, y, width, height) = rect.to_pixels(image);
        assert!((x - 40.0).abs() < 1e-3);
        assert!((y - 100.0).abs() < 1e-3);
        assert!((width - 80.0).abs() < 1e-3);
        assert!((height - 100.0).abs() < 1e-3);
    }

    #[test]
    fn invalid_and_offimage_geometry_is_rejected() {
        assert!(
            NormalizedRect::new(0.1, 0.1, 0.0, 0.2)
                .clamp_unit()
                .is_none()
        );
        assert!(
            NormalizedRect::new(f32::NAN, 0.1, 0.2, 0.2)
                .clamp_unit()
                .is_none()
        );
        assert!(
            NormalizedRect::new(-0.5, 0.1, 0.2, 0.2)
                .clamp_unit()
                .is_none()
        );
        assert!(
            NormalizedRect::new(1.5, 0.1, 0.2, 0.2)
                .clamp_unit()
                .is_none()
        );

        let clamped = NormalizedRect::new(-0.1, -0.1, 0.4, 0.4)
            .clamp_unit()
            .unwrap();
        assert!((clamped.x - 0.0).abs() < 1e-6);
        assert!((clamped.width - 0.3).abs() < 1e-6);

        let mut face = observation(NormalizedRect::new(0.1, 0.1, 0.2, 0.2));
        assert!(face.is_storable());
        face.detection_score = 1.4;
        assert!(!face.is_storable());
        // A detector must not be able to persist a region the UI cannot draw.
        face.detection_score = 0.9;
        face.bbox = NormalizedRect::new(0.0, 0.0, 2.0, 2.0);
        assert!(
            face.is_storable(),
            "an over-large rect is clamped, not fatal"
        );
    }

    #[test]
    fn iou_is_zero_for_disjoint_regions() {
        let a = NormalizedRect::new(0.0, 0.0, 0.2, 0.2);
        let b = NormalizedRect::new(0.5, 0.5, 0.2, 0.2);
        assert_eq!(a.iou(b), 0.0);
        let c = NormalizedRect::new(0.1, 0.1, 0.2, 0.2);
        // Half-overlap: intersection 0.01, union 0.07.
        assert!((a.iou(c) - 0.01 / 0.07).abs() < 1e-6);
    }

    #[test]
    fn custom_sensitivity_uses_the_numeric_threshold() {
        let mut settings = FaceAnalyzerSettings::default();
        assert_eq!(settings.effective_match_threshold(), 0.363);
        settings.match_sensitivity = FaceMatchSensitivity::Strict;
        assert_eq!(settings.effective_match_threshold(), 0.50);
        settings.match_sensitivity = FaceMatchSensitivity::Custom;
        settings.match_threshold = 0.42;
        assert_eq!(settings.effective_match_threshold(), 0.42);
    }

    #[test]
    fn small_face_tiling_defaults_on_and_decodes_absent_fields_as_on() {
        assert!(FaceAnalyzerSettings::default().detect_small_faces);
        // An older stored settings blob has no field and must keep the default.
        let decoded: FaceAnalyzerSettings = serde_json::from_str(
            r#"{"detectionConfidence":0.9,"nmsThreshold":0.3,
                "maxFacesPerAsset":64,"minFacePixels":24,"matchSensitivity":"balanced",
                "matchThreshold":0.363,"clusterThreshold":0.363}"#,
        )
        .unwrap();
        assert!(decoded.detect_small_faces);
    }

    #[test]
    fn sanitized_settings_stay_in_range() {
        let settings = FaceAnalyzerSettings {
            detection_confidence: 5.0,
            nms_threshold: -1.0,
            max_faces_per_asset: 0,
            min_face_pixels: 0,
            match_sensitivity: FaceMatchSensitivity::Loose,
            match_threshold: 9.0,
            auto_accept_threshold: Some(-3.0),
            cluster_threshold: 4.0,
            detect_small_faces: true,
        }
        .sanitized();
        assert_eq!(settings.detection_confidence, 0.99);
        assert_eq!(settings.nms_threshold, 0.0);
        assert_eq!(settings.max_faces_per_asset, 1);
        assert_eq!(settings.min_face_pixels, 8);
        assert_eq!(settings.match_threshold, 1.0);
        assert_eq!(settings.cluster_threshold, 1.0);
        // Auto-accept can never fall below the candidate threshold.
        assert_eq!(settings.auto_accept_threshold, Some(0.25));
    }

    #[test]
    fn recompute_scopes_are_ordered() {
        assert!(FaceRecomputeScope::Detection.invalidates_detection());
        assert!(FaceRecomputeScope::Detection.invalidates_embedding());
        assert!(FaceRecomputeScope::Detection.invalidates_clustering());
        assert!(!FaceRecomputeScope::Embedding.invalidates_detection());
        assert!(FaceRecomputeScope::Embedding.invalidates_embedding());
        assert!(!FaceRecomputeScope::Matching.invalidates_detection());
        assert!(!FaceRecomputeScope::Matching.invalidates_embedding());
        assert!(!FaceRecomputeScope::Matching.invalidates_clustering());
    }

    #[test]
    fn decisions_carry_the_person_only_when_they_concern_one() {
        let confirm = FaceDecision::ConfirmPerson {
            person_id: "p-1".into(),
        };
        assert_eq!(confirm.kind(), "confirm_person");
        assert_eq!(confirm.person_id(), Some("p-1"));
        assert_eq!(FaceDecision::NotFace.kind(), "not_face");
        assert_eq!(FaceDecision::NotFace.person_id(), None);
    }

    #[test]
    fn decisions_use_the_camel_case_field_names_the_frontend_sends() {
        // The loupe overlay and the workbench build the IPC payload as
        // `{ decision: "confirmPerson", personId }`. A tagged enum camel-cases
        // its variants with `rename_all`, but its struct-variant fields need
        // `rename_all_fields`; without it every confirm/reject answer fails to
        // deserialize and the button silently does nothing.
        let confirm: FaceDecision = serde_json::from_value(serde_json::json!({
            "decision": "confirmPerson",
            "personId": "p-1",
        }))
        .unwrap();
        assert_eq!(confirm.person_id(), Some("p-1"));

        let reject: FaceDecision = serde_json::from_value(serde_json::json!({
            "decision": "rejectPerson",
            "personId": "p-2",
        }))
        .unwrap();
        assert_eq!(reject.person_id(), Some("p-2"));

        let json = serde_json::to_value(&confirm).unwrap();
        assert_eq!(json["personId"], "p-1");
        assert!(json.get("person_id").is_none());

        // A `people.json` written while the fields were still snake-cased must
        // keep loading: the store quarantines a document it cannot parse.
        let legacy: FaceDecision = serde_json::from_value(serde_json::json!({
            "decision": "confirmPerson",
            "person_id": "p-3",
        }))
        .unwrap();
        assert_eq!(legacy.person_id(), Some("p-3"));
    }

    #[test]
    fn review_state_names_round_trip() {
        for state in [
            FaceReviewState::Unknown,
            FaceReviewState::Pending,
            FaceReviewState::Confirmed,
            FaceReviewState::Rejected,
            FaceReviewState::NotFace,
        ] {
            assert_eq!(FaceReviewState::parse(state.as_str()), Some(state));
        }
        assert_eq!(FaceReviewState::parse("bogus"), None);
    }

    #[test]
    fn revision_tokens_change_with_the_source_and_match_the_file() {
        assert_eq!(
            face_source_revision(100, 200),
            format!("100:200:{FACE_REVISION_VERSION}")
        );
        assert_ne!(
            face_source_revision(100, 200),
            face_source_revision(101, 200)
        );
        assert_ne!(
            face_source_revision(100, 200),
            face_source_revision(100, 201)
        );

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("a.jpg");
        std::fs::write(&path, b"aaaa").unwrap();
        let revision = face_source_revision_for_path(&path).unwrap();
        assert!(revision.ends_with(&format!(":4:{FACE_REVISION_VERSION}")));
        assert_eq!(revision, face_source_revision_for_path(&path).unwrap());
        std::fs::write(&path, b"aaaaaa").unwrap();
        assert_ne!(revision, face_source_revision_for_path(&path).unwrap());
        assert!(face_source_revision_for_path(&directory.path().join("absent.jpg")).is_err());
    }

    #[test]
    fn an_analysis_request_prefers_explicit_paths_over_a_directory() {
        let request = FaceAnalysisRequest {
            paths: vec![PathBuf::from("/photos/a.jpg")],
            root_path: Some(PathBuf::from("/photos")),
            directory: Some(PathBuf::from("/photos/sub")),
            force: false,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["paths"][0], "/photos/a.jpg");
        assert_eq!(json["directory"], "/photos/sub");

        // A whole-library request omits both scopes rather than sending nulls.
        let library = serde_json::to_value(FaceAnalysisRequest::library(true)).unwrap();
        assert!(library.get("directory").is_none());
        assert!(library.get("rootPath").is_none());
        assert_eq!(library["force"], true);
        assert!(FaceAnalysisRequest::default().paths.is_empty());
    }

    #[test]
    fn a_decision_record_may_carry_the_proposed_score() {
        let record = DecisionRecord {
            observation_id: Some("obs-1".into()),
            asset_id: "asset".into(),
            asset_path: PathBuf::from("/photos/a.jpg"),
            region: NormalizedRect::new(0.1, 0.1, 0.2, 0.2),
            decision: FaceDecision::ConfirmPerson {
                person_id: "p1".into(),
            },
            created_at_ms: 1,
            proposed_similarity: Some(0.52),
        };
        let json = serde_json::to_value(&record).unwrap();
        let stored = json["proposedSimilarity"].as_f64().unwrap();
        assert!((stored - 0.52).abs() < 1e-6);

        // An older record without the field still loads, and one that has it
        // written by a newer build is not rejected.
        let legacy: DecisionRecord = serde_json::from_value(serde_json::json!({
            "assetId": "asset",
            "assetPath": "/photos/a.jpg",
            "region": {"x": 0.1, "y": 0.1, "width": 0.2, "height": 0.2},
            "decision": {"decision": "notFace"},
            "createdAtMs": 1
        }))
        .unwrap();
        assert_eq!(legacy.proposed_similarity, None);
    }

    #[test]
    fn review_filter_names_are_stable() {
        assert_eq!(FaceReviewFilter::default(), FaceReviewFilter::All);
        assert_eq!(FaceReviewFilter::Pending.as_str(), "pending");
        assert_eq!(FaceReviewFilter::Rejected.as_str(), "rejected");

        // The workbench sends the name straight from `types.ts`, so the serde
        // name and `as_str` must agree with the TypeScript union.
        let unreviewed: FaceReviewFilter =
            serde_json::from_value(serde_json::json!("unreviewed")).unwrap();
        assert_eq!(unreviewed, FaceReviewFilter::Unreviewed);
        assert_eq!(unreviewed.as_str(), "unreviewed");
    }

    #[test]
    fn person_serializes_without_optional_links() {
        let person = Person {
            person_id: "p-1".into(),
            display_name: "Alice".into(),
            linked_tag_id: None,
            created_at_ms: 1,
            updated_at_ms: 2,
            face_count: 3,
            cover_observation_id: None,
        };
        let json = serde_json::to_value(&person).unwrap();
        assert_eq!(json["personId"], "p-1");
        assert_eq!(json["faceCount"], 3);
        assert!(json.get("linkedTagId").is_none());
    }
}
