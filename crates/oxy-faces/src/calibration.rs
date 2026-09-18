//! Threshold calibration from the user's own accept/reject history.
//!
//! The report is explicit that a cosine threshold is a property of a model, a
//! distance measure, and an evaluation set — not a universal constant. The only
//! defensible way to pick one for a real library is to measure how the matcher
//! behaves on that library, which means recording the score behind every user
//! answer and reading the two distributions back.
//!
//! # What the samples can and cannot say
//!
//! A score is recorded when the user answers a *proposal*. The score of a
//! (face, person) pair does not depend on the threshold — only whether a
//! candidate was created does — so the scores themselves are clean. The
//! *sample set* is not: nothing below the current threshold was ever proposed,
//! so it was never accepted or rejected. A recommendation is therefore a
//! refinement of the current setting. Faces the user named from scratch, with
//! no proposal, contribute nothing, which is deliberate: there is no score to
//! learn from.

/// Minimum accepted and rejected examples before a threshold is suggested.
///
/// Two or three points would let a single misclick move the recommendation by
/// more than the model's own noise.
pub const MIN_SAMPLES_PER_CLASS: usize = 3;

/// Suggests a candidate threshold from measured accept/reject scores.
///
/// Returns `None` while either class has fewer than [`MIN_SAMPLES_PER_CLASS`]
/// examples. Otherwise the threshold maximizing Youden's J (true-positive rate
/// minus false-positive rate, treating "accepted" as the positive class) is
/// returned; among equally good candidates the middle of the widest gap is
/// chosen so the setting does not sit on top of a data point.
pub fn recommend_threshold(accepted: &[f32], rejected: &[f32]) -> Option<f32> {
    let accepted: Vec<f32> = accepted.iter().copied().filter(|v| v.is_finite()).collect();
    let rejected: Vec<f32> = rejected.iter().copied().filter(|v| v.is_finite()).collect();
    if accepted.len() < MIN_SAMPLES_PER_CLASS || rejected.len() < MIN_SAMPLES_PER_CLASS {
        return None;
    }

    // Candidate thresholds are midpoints between neighbouring observed scores.
    // A threshold never needs to sit exactly on a sample: the comparison is
    // inclusive, so any value in the gap behaves identically.
    let mut scores: Vec<f32> = accepted.iter().chain(rejected.iter()).copied().collect();
    scores.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    scores.dedup_by(|left, right| (*left - *right).abs() < 1e-6);

    let mut candidates: Vec<f32> = Vec::new();
    for window in scores.windows(2) {
        candidates.push((window[0] + window[1]) / 2.0);
    }
    // Also consider below the lowest and above the highest score, so a fully
    // separated population can be split at either end.
    let lowest = *scores.first()?;
    let highest = *scores.last()?;
    candidates.push(lowest - 0.01);
    candidates.push(highest + 0.01);
    candidates.retain(|value| (-1.0..=1.0).contains(value));

    let total_accepted = accepted.len() as f32;
    let total_rejected = rejected.len() as f32;
    let mut best: Option<(f32, f32)> = None; // (J, ambiguity)
    for candidate in candidates {
        let true_positive = accepted.iter().filter(|score| **score >= candidate).count() as f32;
        let false_positive = rejected.iter().filter(|score| **score >= candidate).count() as f32;
        let j = true_positive / total_accepted - false_positive / total_rejected;
        // Prefer the candidate furthest from any observed score: with equal J,
        // that is the most robust place to sit.
        let distance = scores
            .iter()
            .map(|score| (score - candidate).abs())
            .fold(f32::INFINITY, f32::min);
        let better = match best {
            None => true,
            Some((best_j, best_distance)) => {
                j > best_j + 1e-6 || ((j - best_j).abs() <= 1e-6 && distance > best_distance)
            }
        };
        if better {
            best = Some((j, distance));
        }
    }
    best.map(|(_, _)| {
        // Recompute the winning candidate explicitly rather than carrying it in
        // the tuple, so the tie-break rule stays readable.
        best_candidate(&accepted, &rejected, &scores)
    })
}

/// The candidate that wins under the same rule used by [`recommend_threshold`].
fn best_candidate(accepted: &[f32], rejected: &[f32], scores: &[f32]) -> f32 {
    let mut candidates: Vec<f32> = scores.windows(2).map(|w| (w[0] + w[1]) / 2.0).collect();
    if let (Some(lowest), Some(highest)) = (scores.first(), scores.last()) {
        candidates.push(lowest - 0.01);
        candidates.push(highest + 0.01);
    }
    candidates.retain(|value| (-1.0..=1.0).contains(value));

    let total_accepted = accepted.len() as f32;
    let total_rejected = rejected.len() as f32;
    let mut winner = candidates.first().copied().unwrap_or(0.0);
    let mut best_j = f32::NEG_INFINITY;
    let mut best_distance = f32::NEG_INFINITY;
    for candidate in candidates {
        let true_positive = accepted.iter().filter(|score| **score >= candidate).count() as f32;
        let false_positive = rejected.iter().filter(|score| **score >= candidate).count() as f32;
        let j = true_positive / total_accepted - false_positive / total_rejected;
        let distance = scores
            .iter()
            .map(|score| (score - candidate).abs())
            .fold(f32::INFINITY, f32::min);
        if j > best_j + 1e-6 || ((j - best_j).abs() <= 1e-6 && distance > best_distance) {
            winner = candidate;
            best_j = j;
            best_distance = distance;
        }
    }
    winner
}

/// True when every accepted score is strictly above every rejected score.
pub fn populations_separate(accepted: &[f32], rejected: &[f32]) -> bool {
    let Some(lowest_accepted) = accepted
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .reduce(f32::min)
    else {
        return false;
    };
    let Some(highest_rejected) = rejected
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .reduce(f32::max)
    else {
        return false;
    };
    lowest_accepted > highest_rejected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_few_samples_produce_no_recommendation() {
        assert_eq!(recommend_threshold(&[], &[]), None);
        assert_eq!(recommend_threshold(&[0.9, 0.8], &[0.1, 0.2]), None);
        // Enough accepted but not enough rejected.
        assert_eq!(recommend_threshold(&[0.9, 0.8, 0.85], &[0.1, 0.2]), None);
        // Exactly the minimum is enough.
        assert!(recommend_threshold(&[0.9, 0.8, 0.85], &[0.1, 0.2, 0.15]).is_some());
    }

    #[test]
    fn a_clean_separation_lands_between_the_two_populations() {
        let accepted = [0.72, 0.68, 0.81, 0.75];
        let rejected = [0.21, 0.34, 0.28, 0.30];
        let threshold = recommend_threshold(&accepted, &rejected).unwrap();
        assert!(
            threshold > 0.34 && threshold < 0.68,
            "threshold {threshold} must sit in the gap"
        );
        assert!(populations_separate(&accepted, &rejected));
    }

    #[test]
    fn an_overlapping_population_still_separates_as_well_as_possible() {
        // Three accepted high, one accepted low; three rejected low, one high.
        // One sample has to be wrong whichever way the line is drawn, and
        // Youden's J spends that error on the rejected side: a missed real face
        // is the mistake the user notices, so the accepted outlier is kept.
        let accepted = [0.80, 0.75, 0.70, 0.30];
        let rejected = [0.25, 0.20, 0.22, 0.78];
        let threshold = recommend_threshold(&accepted, &rejected).unwrap();
        assert!(
            threshold > 0.25 && threshold < 0.30,
            "the split keeps the accepted sample and drops the rejected ones, got {threshold}"
        );
        let errors = accepted.iter().filter(|score| **score < threshold).count()
            + rejected.iter().filter(|score| **score >= threshold).count();
        assert_eq!(errors, 1, "one sample is necessarily misclassified");
        assert!(!populations_separate(&accepted, &rejected));
    }

    #[test]
    fn the_recommendation_is_not_placed_on_a_data_point() {
        let accepted = [0.90, 1.0, 0.95];
        let rejected = [0.10, 0.05, 0.20];
        let threshold = recommend_threshold(&accepted, &rejected).unwrap();
        for score in accepted.iter().chain(rejected.iter()) {
            assert!(
                (score - threshold).abs() > 1e-3,
                "threshold {threshold} sits on {score}"
            );
        }
    }

    #[test]
    fn the_recommendation_accepts_every_measured_pair_and_rejects_none() {
        // The whole point: after applying it, every example the user accepted
        // is still a candidate and none of the rejected ones is.
        let accepted = [0.61, 0.55, 0.70];
        let rejected = [0.12, 0.35, 0.20];
        let threshold = recommend_threshold(&accepted, &rejected).unwrap();
        assert!(accepted.iter().all(|score| *score >= threshold));
        assert!(rejected.iter().all(|score| *score < threshold));
    }

    #[test]
    fn non_finite_scores_are_ignored() {
        let threshold = recommend_threshold(
            &[f32::NAN, 0.9, 0.8, 0.85],
            &[f32::INFINITY, 0.1, 0.2, 0.15],
        )
        .unwrap();
        assert!(threshold > 0.2 && threshold < 0.8);
        assert_eq!(recommend_threshold(&[f32::NAN; 4], &[0.1, 0.2, 0.3]), None);
    }

    #[test]
    fn a_tight_gap_between_populations_is_still_usable() {
        let accepted = [0.4001, 0.4002, 0.4000];
        let rejected = [0.3998, 0.3999, 0.3997];
        let threshold = recommend_threshold(&accepted, &rejected).unwrap();
        assert!(threshold > 0.3999 && threshold < 0.4000);
        assert!(populations_separate(&accepted, &rejected));
    }
}
