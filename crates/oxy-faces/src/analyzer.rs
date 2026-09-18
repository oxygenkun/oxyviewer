//! The analyzer facade: pixels in, observations and embeddings out.
//!
//! One [`FaceAnalyzer`] owns a detector and an embedder plus the
//! detection-affecting settings. Everything it produces is rebuildable machine
//! output; user decisions arrive later through the Host person domain.

use std::path::{Path, PathBuf};

use oxy_domain::{
    FaceAnalyzerSettings, FaceObservation, NormalizedPoint, NormalizedRect, PixelSize,
    face_source_revision_for_path,
};
use sha2::{Digest, Sha256};

use crate::FaceError;
use crate::align::align_face;
use crate::image::{RgbImage, analysis_views};
use crate::matcher::normalize_embedding;
use crate::sface::{ALIGNED_SIZE, EMBEDDING_DIM, SFaceEmbedder};
use crate::yunet::YuNetDetector;

/// Where the analyzer finds its models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceModelPaths {
    pub detector: PathBuf,
    pub embedder: PathBuf,
    /// Fixed spatial input of the exported detector graph.
    pub detector_input_size: u32,
}

impl FaceModelPaths {
    pub fn new(
        detector: impl Into<PathBuf>,
        embedder: impl Into<PathBuf>,
        detector_input_size: u32,
    ) -> Self {
        Self {
            detector: detector.into(),
            embedder: embedder.into(),
            detector_input_size,
        }
    }
}

/// One detected face plus its embedding.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalyzedFace {
    pub observation: FaceObservation,
    /// Raw (unnormalized) SFace embedding. Persisted as a BLOB, never as JSON.
    pub embedding: Vec<f32>,
}

/// Detects faces and extracts embeddings from decoded pixels.
pub struct FaceAnalyzer {
    detector: YuNetDetector,
    embedder: SFaceEmbedder,
    settings: FaceAnalyzerSettings,
}

impl std::fmt::Debug for FaceAnalyzer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FaceAnalyzer")
            .field("detector", &self.detector)
            .field("embedder", &self.embedder)
            .field("settings", &self.settings)
            .finish()
    }
}

impl FaceAnalyzer {
    /// Loads both models and validates their layouts with a probe inference.
    pub fn load(paths: &FaceModelPaths, settings: FaceAnalyzerSettings) -> Result<Self, FaceError> {
        let settings = settings.sanitized();
        Ok(Self {
            detector: YuNetDetector::load(&paths.detector, paths.detector_input_size)?,
            embedder: SFaceEmbedder::load(&paths.embedder)?,
            settings,
        })
    }

    pub fn settings(&self) -> &FaceAnalyzerSettings {
        &self.settings
    }

    /// Detection fingerprint including every detection-affecting parameter, so
    /// changing the confidence threshold invalidates stored detections while
    /// changing only the match threshold does not.
    pub fn detector_fingerprint(&self) -> String {
        format!(
            "{}/full-v1/conf{:.3}/nms{:.3}/minpx{}/tiles{}",
            self.detector.fingerprint(),
            self.settings.detection_confidence,
            self.settings.nms_threshold,
            self.settings.min_face_pixels,
            u8::from(self.settings.detect_small_faces),
        )
    }

    pub fn embedder_fingerprint(&self) -> &str {
        self.embedder.fingerprint()
    }

    pub fn embedding_dim(&self) -> usize {
        EMBEDDING_DIM
    }

    /// Analyzes one decoded image.
    ///
    /// `source_revision` identifies the exact asset revision the pixels came
    /// from. It is stored on every observation so the Host can reject a result
    /// whose asset changed mid-analysis instead of overwriting newer work.
    /// Analyzes an image and refuses the result if the source changed meanwhile.
    ///
    /// `revision` is what the caller observed *before* decoding, so the pixels
    /// and the revision describe the same bytes. This runs the analysis and then
    /// re-reads the file: if it changed, `Ok(None)` means "discard this result"
    /// rather than storing observations for bytes that no longer exist.
    ///
    /// The check lives here rather than in the caller so the guarantee travels
    /// with the analyzer instead of being re-implemented by every host.
    pub fn analyze_verified(
        &self,
        asset_id: &str,
        asset_path: &Path,
        revision: &str,
        image: &RgbImage,
    ) -> Result<Option<Vec<AnalyzedFace>>, FaceError> {
        let analyzed = self.analyze(asset_id, asset_path, revision, image)?;
        let current = face_source_revision_for_path(asset_path).map_err(|error| {
            FaceError::SourceRevision {
                path: asset_path.to_path_buf(),
                message: error.to_string(),
            }
        })?;
        if current != revision {
            return Ok(None);
        }
        Ok(Some(analyzed))
    }

    pub fn analyze(
        &self,
        asset_id: &str,
        asset_path: &Path,
        source_revision: &str,
        image: &RgbImage,
    ) -> Result<Vec<AnalyzedFace>, FaceError> {
        let views = analysis_views(
            image,
            self.detector.input_size(),
            self.settings.detect_small_faces,
        );
        let detector_fingerprint = self.detector_fingerprint();
        let size = PixelSize {
            width: image.width(),
            height: image.height(),
        };

        // Collect every view's detections in source coordinates, then merge.
        // Tiles overlap, so the same face is found several times and must be
        // reduced to one observation with the best score.
        let mut candidates: Vec<Candidate> = Vec::new();
        for view in &views {
            let detections = self.detector.detect(&view.boxed, &self.settings)?;
            for detection in detections {
                let (x, y, width, height) = view.to_source_rect(
                    detection.x,
                    detection.y,
                    detection.width,
                    detection.height,
                );
                let Some(bbox) =
                    NormalizedRect::from_pixels(x, y, width, height, size).clamp_unit()
                else {
                    continue;
                };
                let landmarks = detection.landmarks.map(|point| {
                    let (px, py) = view.to_source_point(point[0], point[1]);
                    [px, py]
                });
                candidates.push(Candidate {
                    bbox,
                    landmarks,
                    score: detection.score,
                });
            }
        }

        let merged = merge_candidates(
            candidates,
            self.settings.nms_threshold,
            self.settings.max_faces_per_asset as usize,
        );

        let mut analyzed = Vec::with_capacity(merged.len());
        for (index, candidate) in merged.iter().enumerate() {
            // Alignment runs on the full-resolution source, not a view, so a
            // small face is not embedded from an upscaled crop.
            let Some(aligned) = align_face(image, &candidate.landmarks, ALIGNED_SIZE) else {
                continue;
            };
            let embedding = self.embedder.embed(&aligned)?;
            if embedding.len() != EMBEDDING_DIM {
                return Err(FaceError::UnexpectedEmbedding {
                    observation_id: format!("{asset_id}#{index}"),
                    found: embedding.len(),
                    expected: EMBEDDING_DIM,
                });
            }

            let landmarks: Vec<NormalizedPoint> = candidate
                .landmarks
                .iter()
                .map(|point| {
                    NormalizedPoint::new(
                        (point[0] / size.width as f32).clamp(0.0, 1.0),
                        (point[1] / size.height as f32).clamp(0.0, 1.0),
                    )
                })
                .collect();

            let observation = FaceObservation {
                observation_id: observation_id(
                    asset_id,
                    source_revision,
                    &detector_fingerprint,
                    index as u32,
                ),
                asset_id: asset_id.to_string(),
                asset_path: asset_path.to_path_buf(),
                source_revision: source_revision.to_string(),
                local_index: index as u32,
                bbox: candidate.bbox,
                landmarks,
                detection_score: candidate.score,
                detector_fingerprint: detector_fingerprint.clone(),
            };
            if !observation.is_storable() {
                continue;
            }
            analyzed.push(AnalyzedFace {
                observation,
                embedding: normalize_embedding(&embedding),
            });
        }
        Ok(analyzed)
    }
}

/// One detection in normalized source coordinates.
struct Candidate {
    bbox: NormalizedRect,
    landmarks: [[f32; 2]; 5],
    score: f32,
}

/// Reduces overlapping detections from several views to one per face.
///
/// The sort is by score, then by position, rather than by the order views
/// happened to run in. `local_index` and therefore the observation id are
/// derived from this order, so a stable sort is what keeps repeated analysis
/// idempotent.
fn merge_candidates(
    mut candidates: Vec<Candidate>,
    nms_threshold: f32,
    max_faces: usize,
) -> Vec<Candidate> {
    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                left.bbox
                    .x
                    .partial_cmp(&right.bbox.x)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                left.bbox
                    .y
                    .partial_cmp(&right.bbox.y)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut kept: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        if kept.len() >= max_faces {
            break;
        }
        let overlaps = kept
            .iter()
            .any(|existing| existing.bbox.iou(candidate.bbox) > nms_threshold);
        if !overlaps {
            kept.push(candidate);
        }
    }
    kept
}

/// Deterministic observation id. Analysis is routinely re-run over the same
/// asset; a content-derived id keeps re-runs idempotent instead of creating a
/// second row for the same face.
fn observation_id(
    asset_id: &str,
    source_revision: &str,
    detector_fingerprint: &str,
    local_index: u32,
) -> String {
    let mut hasher = Sha256::new();
    for part in [asset_id, source_revision, detector_fingerprint] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hasher.update(local_index.to_le_bytes());
    let digest = hasher.finalize();
    let hex: String = digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("face-{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::RgbImage;

    fn candidate(x: f32, y: f32, score: f32) -> Candidate {
        Candidate {
            bbox: NormalizedRect::new(x, y, 0.1, 0.1),
            landmarks: [[0.0; 2]; 5],
            score,
        }
    }

    fn solid(width: u32, height: u32) -> RgbImage {
        RgbImage::new(width, height, vec![0; width as usize * height as usize * 3]).unwrap()
    }

    #[test]
    fn merging_deduplicates_a_face_found_by_several_views() {
        // The same face found by the whole frame and by two overlapping tiles.
        let merged = merge_candidates(
            vec![
                candidate(0.40, 0.40, 0.95),
                candidate(0.402, 0.398, 0.91),
                candidate(0.60, 0.10, 0.80),
            ],
            0.3,
            64,
        );
        assert_eq!(merged.len(), 2);
        assert!((merged[0].score - 0.95).abs() < 1e-6, "the best score wins");
    }

    #[test]
    fn merging_is_deterministic_for_equal_scores() {
        let forward = merge_candidates(
            vec![candidate(0.6, 0.1, 0.8), candidate(0.1, 0.1, 0.8)],
            0.3,
            64,
        );
        let reversed = merge_candidates(
            vec![candidate(0.1, 0.1, 0.8), candidate(0.6, 0.1, 0.8)],
            0.3,
            64,
        );
        let positions = |items: &[Candidate]| {
            items
                .iter()
                .map(|item| (item.bbox.x, item.bbox.y))
                .collect::<Vec<_>>()
        };
        assert_eq!(positions(&forward), positions(&reversed));
        assert_eq!(positions(&forward), vec![(0.1, 0.1), (0.6, 0.1)]);
    }

    #[test]
    fn merging_respects_the_face_limit() {
        let merged = merge_candidates(
            vec![
                candidate(0.1, 0.1, 0.9),
                candidate(0.4, 0.1, 0.8),
                candidate(0.7, 0.1, 0.7),
            ],
            0.3,
            2,
        );
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn a_small_image_gets_one_view_and_a_large_one_gets_tiles() {
        let small = analysis_views(&solid(800, 600), 640, true);
        assert_eq!(small.len(), 1, "no downscaling worth tiling for");
        assert_eq!((small[0].origin_x, small[0].origin_y), (0.0, 0.0));

        // 2400 is under 4x 640, so a 2x2 grid plus the frame.
        let medium = analysis_views(&solid(2400, 1600), 640, true);
        assert_eq!(medium.len(), 5);
        // 4000 is over 4x 640, so the grid grows to 3x3.
        let large = analysis_views(&solid(4000, 3000), 640, true);
        assert_eq!(large.len(), 10);
        // Disabling tiling always leaves exactly the whole frame.
        assert_eq!(analysis_views(&solid(4000, 3000), 640, false).len(), 1);
    }

    #[test]
    fn tiles_cover_the_whole_image() {
        let source = solid(2400, 1600);
        let views = analysis_views(&source, 640, true);
        let mut covered_x = vec![false; source.width() as usize];
        let mut covered_y = vec![false; source.height() as usize];
        for view in views.iter().skip(1) {
            // `source_width` on a view's letterbox is the crop width, because
            // the view was letterboxed from the cropped image.
            let width = view.boxed.source_width;
            let height = view.boxed.source_height;
            let left = view.origin_x as u32;
            let top = view.origin_y as u32;
            for x in left..(left + width).min(source.width()) {
                covered_x[x as usize] = true;
            }
            for y in top..(top + height).min(source.height()) {
                covered_y[y as usize] = true;
            }
        }
        assert!(
            covered_x.iter().all(|covered| *covered) && covered_y.iter().all(|covered| *covered),
            "every pixel must fall inside at least one tile"
        );
    }

    #[test]
    fn a_view_maps_its_detections_back_to_source_coordinates() {
        let source = solid(2400, 1600);
        let views = analysis_views(&source, 640, true);
        let tile = views
            .iter()
            .find(|view| view.origin_x > 0.0 && view.origin_y > 0.0)
            .expect("an interior tile exists");
        // The tile's own (0, 0) maps to its origin in the source image.
        let (x, y, _, _) = tile.to_source_rect(0.0, 0.0, 1.0, 1.0);
        assert!((x - tile.origin_x).abs() < 0.5);
        assert!((y - tile.origin_y).abs() < 0.5);
        let (px, py) = tile.to_source_point(0.0, 0.0);
        assert!((px - tile.origin_x).abs() < 0.5 && (py - tile.origin_y).abs() < 0.5);
    }

    #[test]
    fn observation_ids_are_content_derived_and_stable() {
        let first = observation_id("asset", "rev", "detector", 3);
        assert_eq!(first, observation_id("asset", "rev", "detector", 3));
        assert_ne!(first, observation_id("asset", "rev", "detector", 4));
        assert_ne!(first, observation_id("asset", "rev2", "detector", 3));
        assert_ne!(first, observation_id("other", "rev", "detector", 3));
        assert_ne!(first, observation_id("asset", "rev", "detector-v2", 3));
        assert!(first.starts_with("face-"));
    }
}
