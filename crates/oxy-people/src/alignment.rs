//! Conservative correspondence between model detections and manual anchors.
//! This produces evidence only; it never changes reviews or identities.

use crate::geometry::valid_box;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ManualAnchor {
    pub id: String,
    pub source_identity_revision: Option<String>,
    pub face_box: Option<[f64; 4]>,
    pub body_box: Option<[f64; 4]>,
    pub needs_review: bool,
}

#[derive(Debug, Clone)]
pub struct DetectedAnchor {
    pub id: String,
    pub source_identity_revision: String,
    pub face_box: Option<[f64; 4]>,
    pub body_box: Option<[f64; 4]>,
}

#[derive(Debug, Clone, Copy)]
pub struct AlignmentPolicy {
    pub min_face_iou: f64,
    pub min_body_iou: f64,
    pub min_associated_body_iou: f64,
    pub uniqueness_margin: f64,
}

impl Default for AlignmentPolicy {
    fn default() -> Self {
        Self {
            min_face_iou: 0.75,
            min_body_iou: 0.8,
            min_associated_body_iou: 0.5,
            uniqueness_margin: 0.15,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignmentConflictReason {
    SourceUnverified,
    SourceChanged,
    NeedsReview,
    NoMatchingDetection,
    AmbiguousGeometry,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlignmentMatch {
    pub manual_id: String,
    pub detected_id: String,
    pub geometric_score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlignmentConflict {
    pub manual_id: String,
    pub reason: AlignmentConflictReason,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlignmentResult {
    pub matches: Vec<AlignmentMatch>,
    pub conflicts: Vec<AlignmentConflict>,
    pub unmatched_detected_ids: Vec<String>,
}

#[derive(Debug, Error)]
pub enum AlignmentError {
    #[error("invalid or duplicated person instance anchor")]
    InvalidAnchor,
    #[error("invalid alignment policy")]
    InvalidPolicy,
}

/// Derives a cache identity from source, producer, and normalized geometry.
/// Detector ordering is deliberately absent. A changed box may get a new ID;
/// `align_instances` handles conservative correspondence across reruns.
pub fn detection_cache_id(
    source_identity_revision: &str,
    producer_fingerprint: &str,
    face_box: Option<[f64; 4]>,
    body_box: Option<[f64; 4]>,
) -> Result<String, AlignmentError> {
    if source_identity_revision.len() != 64
        || producer_fingerprint.len() != 64
        || !source_identity_revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || !producer_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || (face_box.is_none() && body_box.is_none())
        || !valid_box(face_box)
        || !valid_box(body_box)
    {
        return Err(AlignmentError::InvalidAnchor);
    }
    let mut hasher = Sha256::new();
    hasher.update(b"oxy-person-instance-cache-v1\0");
    hasher.update(source_identity_revision.as_bytes());
    hasher.update([0]);
    hasher.update(producer_fingerprint.as_bytes());
    for (kind, region) in [(b'F', face_box), (b'B', body_box)] {
        hasher.update([kind]);
        if let Some(region) = region {
            hasher.update([1]);
            for coordinate in region {
                hasher.update(((coordinate * 100_000.0).round() as i64).to_le_bytes());
            }
        } else {
            hasher.update([0]);
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn iou(a: [f64; 4], b: [f64; 4]) -> f64 {
    let left = a[0].max(b[0]);
    let top = a[1].max(b[1]);
    let right = (a[0] + a[2]).min(b[0] + b[2]);
    let bottom = (a[1] + a[3]).min(b[1] + b[3]);
    let intersection = (right - left).max(0.0) * (bottom - top).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - intersection;
    if union > 0.0 {
        intersection / union
    } else {
        0.0
    }
}

fn score(manual: &ManualAnchor, detected: &DetectedAnchor, policy: AlignmentPolicy) -> Option<f64> {
    match (manual.face_box, detected.face_box) {
        (Some(face), Some(candidate)) => {
            let overlap = iou(face, candidate);
            if overlap < policy.min_face_iou {
                return None;
            }
            if let (Some(body), Some(candidate_body)) = (manual.body_box, detected.body_box)
                && iou(body, candidate_body) < policy.min_associated_body_iou
            {
                return None;
            }
            Some(overlap)
        }
        (None, None) => {
            let overlap = iou(manual.body_box?, detected.body_box?);
            (overlap >= policy.min_body_iou).then_some(overlap)
        }
        _ => None,
    }
}

/// Accept only mutual, clearly separated best matches. Ambiguity is surfaced
/// instead of assigning a prior positive or negative review to a nearby face.
pub fn align_instances(
    source_identity_revision: &str,
    manual: &[ManualAnchor],
    detected: &[DetectedAnchor],
    policy: AlignmentPolicy,
) -> Result<AlignmentResult, AlignmentError> {
    if source_identity_revision.is_empty()
        || [
            policy.min_face_iou,
            policy.min_body_iou,
            policy.min_associated_body_iou,
            policy.uniqueness_margin,
        ]
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err(AlignmentError::InvalidPolicy);
    }
    let mut ids = HashSet::new();
    if manual.iter().any(|item| {
        item.id.is_empty()
            || !ids.insert(item.id.as_str())
            || !valid_box(item.face_box)
            || !valid_box(item.body_box)
            || (item.face_box.is_none() && item.body_box.is_none())
    }) {
        return Err(AlignmentError::InvalidAnchor);
    }
    ids.clear();
    if detected.iter().any(|item| {
        item.id.is_empty()
            || !ids.insert(item.id.as_str())
            || item.source_identity_revision.is_empty()
            || !valid_box(item.face_box)
            || !valid_box(item.body_box)
            || (item.face_box.is_none() && item.body_box.is_none())
    }) {
        return Err(AlignmentError::InvalidAnchor);
    }
    let mut by_manual: HashMap<&str, Vec<(&str, f64)>> = HashMap::new();
    let mut by_detected: HashMap<&str, Vec<(&str, f64)>> = HashMap::new();
    let mut conflicts = Vec::new();
    for item in manual {
        let reason = if item.needs_review {
            Some(AlignmentConflictReason::NeedsReview)
        } else if item.source_identity_revision.is_none() {
            Some(AlignmentConflictReason::SourceUnverified)
        } else if item.source_identity_revision.as_deref() != Some(source_identity_revision) {
            Some(AlignmentConflictReason::SourceChanged)
        } else {
            None
        };
        if let Some(reason) = reason {
            conflicts.push(AlignmentConflict {
                manual_id: item.id.clone(),
                reason,
            });
            continue;
        }
        for candidate in detected {
            if candidate.source_identity_revision != source_identity_revision {
                continue;
            }
            if let Some(value) = score(item, candidate, policy) {
                by_manual
                    .entry(&item.id)
                    .or_default()
                    .push((&candidate.id, value));
                by_detected
                    .entry(&candidate.id)
                    .or_default()
                    .push((&item.id, value));
            }
        }
    }
    let rank = |candidates: &mut Vec<(&str, f64)>| {
        candidates.sort_unstable_by(|left, right| {
            right.1.total_cmp(&left.1).then_with(|| left.0.cmp(right.0))
        });
    };
    for candidates in by_manual.values_mut() {
        rank(candidates);
    }
    for candidates in by_detected.values_mut() {
        rank(candidates);
    }
    let clear_best = |candidates: &[(&str, f64)]| {
        candidates.first().is_some_and(|best| {
            candidates
                .get(1)
                .is_none_or(|second| best.1 - second.1 >= policy.uniqueness_margin)
        })
    };
    let mut matches = Vec::new();
    for item in manual {
        if conflicts
            .iter()
            .any(|conflict| conflict.manual_id == item.id)
        {
            continue;
        }
        let Some(candidates) = by_manual.get(item.id.as_str()) else {
            conflicts.push(AlignmentConflict {
                manual_id: item.id.clone(),
                reason: AlignmentConflictReason::NoMatchingDetection,
            });
            continue;
        };
        let (detected_id, value) = candidates[0];
        let reverse = &by_detected[detected_id];
        if !clear_best(candidates) || !clear_best(reverse) || reverse[0].0 != item.id {
            conflicts.push(AlignmentConflict {
                manual_id: item.id.clone(),
                reason: AlignmentConflictReason::AmbiguousGeometry,
            });
            continue;
        }
        matches.push(AlignmentMatch {
            manual_id: item.id.clone(),
            detected_id: detected_id.to_owned(),
            geometric_score: value,
        });
    }
    matches.sort_unstable_by(|left, right| left.manual_id.cmp(&right.manual_id));
    conflicts.sort_unstable_by(|left, right| left.manual_id.cmp(&right.manual_id));
    let matched: HashSet<_> = matches
        .iter()
        .map(|item| item.detected_id.as_str())
        .collect();
    let mut unmatched_detected_ids: Vec<_> = detected
        .iter()
        .filter(|item| !matched.contains(item.id.as_str()))
        .map(|item| item.id.clone())
        .collect();
    unmatched_detected_ids.sort_unstable();
    Ok(AlignmentResult {
        matches,
        conflicts,
        unmatched_detected_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual(id: &str, face_box: Option<[f64; 4]>) -> ManualAnchor {
        ManualAnchor {
            id: id.into(),
            source_identity_revision: Some("source-v1".into()),
            face_box,
            body_box: None,
            needs_review: false,
        }
    }

    fn detected(id: &str, face_box: Option<[f64; 4]>) -> DetectedAnchor {
        DetectedAnchor {
            id: id.into(),
            source_identity_revision: "source-v1".into(),
            face_box,
            body_box: None,
        }
    }

    #[test]
    fn matches_only_unique_same_source_face_geometry() {
        let first = [0.1, 0.1, 0.2, 0.2];
        let second = [0.6, 0.2, 0.2, 0.2];
        let result = align_instances(
            "source-v1",
            &[
                manual("human-a", Some(first)),
                manual("human-b", Some(second)),
            ],
            &[
                detected("model-b", Some(second)),
                detected("model-a", Some(first)),
            ],
            AlignmentPolicy::default(),
        )
        .unwrap();
        assert_eq!(result.matches.len(), 2);
        assert_eq!(result.matches[0].detected_id, "model-a");
        assert_eq!(result.matches[1].detected_id, "model-b");
        assert!(result.conflicts.is_empty());
    }

    #[test]
    fn ambiguous_one_to_many_and_many_to_one_stay_unmatched() {
        let box_a = [0.1, 0.1, 0.2, 0.2];
        let one_to_many = align_instances(
            "source-v1",
            &[manual("human", Some(box_a))],
            &[
                detected("model-a", Some(box_a)),
                detected("model-b", Some(box_a)),
            ],
            AlignmentPolicy::default(),
        )
        .unwrap();
        assert!(one_to_many.matches.is_empty());
        assert_eq!(
            one_to_many.conflicts[0].reason,
            AlignmentConflictReason::AmbiguousGeometry
        );
        assert_eq!(one_to_many.unmatched_detected_ids.len(), 2);

        let many_to_one = align_instances(
            "source-v1",
            &[
                manual("human-a", Some(box_a)),
                manual("human-b", Some(box_a)),
            ],
            &[detected("model", Some(box_a))],
            AlignmentPolicy::default(),
        )
        .unwrap();
        assert!(many_to_one.matches.is_empty());
        assert_eq!(many_to_one.conflicts.len(), 2);
    }

    #[test]
    fn unverified_changed_or_pending_manual_anchors_cannot_inherit_reviews() {
        let face = [0.1, 0.1, 0.2, 0.2];
        let mut unverified = manual("unverified", Some(face));
        unverified.source_identity_revision = None;
        let mut changed = manual("changed", Some(face));
        changed.source_identity_revision = Some("older-source".into());
        let mut pending = manual("pending", Some(face));
        pending.needs_review = true;
        let result = align_instances(
            "source-v1",
            &[unverified, changed, pending],
            &[detected("model", Some(face))],
            AlignmentPolicy::default(),
        )
        .unwrap();
        assert!(result.matches.is_empty());
        assert_eq!(result.unmatched_detected_ids, vec!["model"]);
        assert_eq!(
            result
                .conflicts
                .iter()
                .map(|item| item.reason)
                .collect::<Vec<_>>(),
            vec![
                AlignmentConflictReason::SourceChanged,
                AlignmentConflictReason::NeedsReview,
                AlignmentConflictReason::SourceUnverified,
            ]
        );
    }

    #[test]
    fn body_only_requires_body_overlap_and_face_body_association_must_agree() {
        let body = [0.1, 0.1, 0.4, 0.7];
        let mut human = manual("human", None);
        human.body_box = Some(body);
        let mut model = detected("model", None);
        model.body_box = Some(body);
        assert_eq!(
            align_instances(
                "source-v1",
                &[human.clone()],
                &[model],
                AlignmentPolicy::default()
            )
            .unwrap()
            .matches
            .len(),
            1
        );
        human.face_box = Some([0.2, 0.2, 0.1, 0.1]);
        let mut disagreeing = detected("disagreeing", human.face_box);
        disagreeing.body_box = Some([0.6, 0.1, 0.3, 0.7]);
        assert!(
            align_instances(
                "source-v1",
                &[human],
                &[disagreeing],
                AlignmentPolicy::default()
            )
            .unwrap()
            .matches
            .is_empty()
        );
    }

    #[test]
    fn cache_id_uses_geometry_instead_of_detector_position() {
        let source = "a".repeat(64);
        let producer = "b".repeat(64);
        let face = Some([0.1, 0.2, 0.2, 0.2]);
        let first = detection_cache_id(&source, &producer, face, None).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(
            first,
            detection_cache_id(&source, &producer, face, None).unwrap()
        );
        assert_ne!(
            first,
            detection_cache_id(&source, &producer, Some([0.2, 0.2, 0.2, 0.2]), None).unwrap()
        );
        assert_ne!(
            first,
            detection_cache_id(&source, &producer, None, face).unwrap()
        );
        assert!(detection_cache_id("short", &producer, face, None).is_err());
    }
}
