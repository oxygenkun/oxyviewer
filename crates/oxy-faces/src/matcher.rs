//! Person gallery matching.
//!
//! A person is represented by several confirmed, good-quality examples rather
//! than a single centroid: real libraries contain front, profile, childhood,
//! and strong-backlight shots of the same person, and averaging them destroys
//! the very separation the matcher needs.

use oxy_domain::PersonId;

/// Scales an embedding to unit length. A zero vector is returned unchanged so a
/// broken model cannot produce NaNs downstream.
pub fn normalize_embedding(embedding: &[f32]) -> Vec<f32> {
    let norm = embedding
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm <= f64::EPSILON {
        return embedding.to_vec();
    }
    embedding
        .iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect()
}

/// Cosine similarity. Both sides are normalized here so a caller cannot
/// accidentally compare raw embeddings and get a meaningless score.
pub fn similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    similarity_normalized(&normalize_embedding(left), &normalize_embedding(right))
}

/// Dot product of two already-normalized embeddings. Clustering and matching
/// normalize once and then use this to avoid re-normalizing in an inner loop.
pub fn similarity_normalized(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum::<f64>() as f32
}

/// Confirmed examples of one person, already normalized.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonGallery {
    pub person_id: PersonId,
    pub display_name: String,
    pub examples: Vec<Vec<f32>>,
}

impl PersonGallery {
    /// Builds a gallery, normalizing every example and dropping unusable ones.
    pub fn new(
        person_id: impl Into<PersonId>,
        display_name: impl Into<String>,
        examples: impl IntoIterator<Item = Vec<f32>>,
    ) -> Self {
        let examples = examples
            .into_iter()
            .filter(|example| {
                example.iter().all(|value| value.is_finite())
                    && example.iter().any(|value| *value != 0.0)
            })
            .map(|example| normalize_embedding(&example))
            .collect();
        Self {
            person_id: person_id.into(),
            display_name: display_name.into(),
            examples,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.examples.is_empty()
    }

    /// Per-example similarities, highest first.
    pub fn ranked_similarities(&self, query: &[f32]) -> Vec<f32> {
        let mut scores: Vec<f32> = self
            .examples
            .iter()
            .map(|example| similarity(query, example))
            .collect();
        scores.sort_by(|left, right| right.partial_cmp(left).unwrap_or(std::cmp::Ordering::Equal));
        scores
    }
}

/// Similarity window within which a confirmed example counts as agreeing with
/// the best one.
const CONSISTENT_MARGIN: f32 = 0.05;

/// The best proposal for a query embedding.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonMatch {
    pub person_id: PersonId,
    pub display_name: String,
    /// Similarity to the closest confirmed example. This is the decision score.
    pub similarity: f32,
    /// Confirmed examples that scored within [`CONSISTENT_MARGIN`] of the best,
    /// so the UI can say "3 samples agree" instead of showing one lucky match.
    pub consistent_examples: usize,
    /// Total confirmed examples compared.
    pub example_count: usize,
}

impl PersonMatch {
    pub fn is_candidate(&self, threshold: f32) -> bool {
        self.similarity >= threshold
    }
}

/// Finds the person whose confirmed examples best match `query`.
///
/// The decision score is the similarity to the *closest* confirmed example
/// (nearest-neighbour over the gallery), not a centroid and not a mean of the
/// top `k`. A person usually starts with one or two confirmed faces, and
/// averaging a close example with unrelated ones would reject the very match
/// that should become a candidate. Missing a candidate leaves a face unknown,
/// which costs the user a manual review anyway; a wrong candidate goes to the
/// pending queue and costs one click. Recall therefore wins here, and
/// [`PersonMatch::consistent_examples`] exposes whether several samples agreed.
pub fn match_person(query: &[f32], galleries: &[PersonGallery]) -> Option<PersonMatch> {
    match_person_cancellable(query, galleries, || false)
        .ok()
        .flatten()
}

/// Matches with cancellation between galleries and examples.
pub fn match_person_cancellable(
    query: &[f32],
    galleries: &[PersonGallery],
    cancelled: impl Fn() -> bool,
) -> Result<Option<PersonMatch>, crate::FaceError> {
    crate::check_cancelled(&cancelled)?;
    if query.is_empty() || !query.iter().all(|value| value.is_finite()) {
        return Ok(None);
    }
    let normalized = normalize_embedding(query);
    let mut best: Option<PersonMatch> = None;
    for gallery in galleries {
        crate::check_cancelled(&cancelled)?;
        if gallery.is_empty() {
            continue;
        }
        let scores: Vec<f32> = gallery
            .examples
            .iter()
            .map(|example| {
                crate::check_cancelled(&cancelled)?;
                Ok(similarity_normalized(&normalized, example))
            })
            .collect::<Result<_, crate::FaceError>>()?;
        let top = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let candidate = PersonMatch {
            person_id: gallery.person_id.clone(),
            display_name: gallery.display_name.clone(),
            similarity: top,
            consistent_examples: scores
                .iter()
                .filter(|score| **score >= top - CONSISTENT_MARGIN)
                .count(),
            example_count: scores.len(),
        };
        let replace = best
            .as_ref()
            .is_none_or(|current| candidate.similarity > current.similarity);
        if replace {
            best = Some(candidate);
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example(seed: f32, dim: usize) -> Vec<f32> {
        (0..dim)
            .map(|index| ((index as f32 + 1.0) * seed).sin())
            .collect()
    }

    #[test]
    fn similarity_of_identical_embeddings_is_one() {
        let vector = example(1.0, 32);
        assert!((similarity(&vector, &vector) - 1.0).abs() < 1e-5);
        assert!((similarity(&vector, &example(1.0, 32)) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn similarity_of_opposite_embeddings_is_minus_one() {
        let vector = example(2.0, 16);
        let negated: Vec<f32> = vector.iter().map(|value| -value).collect();
        assert!((similarity(&vector, &negated) + 1.0).abs() < 1e-5);
    }

    #[test]
    fn similarity_rejects_mismatched_lengths() {
        assert_eq!(similarity(&[1.0, 2.0], &[1.0]), 0.0);
        assert_eq!(similarity(&[], &[]), 0.0);
    }

    #[test]
    fn matching_prefers_the_gallery_with_the_closest_example() {
        let alice = PersonGallery::new("alice", "Alice", vec![example(1.0, 32)]);
        let bob = PersonGallery::new("bob", "Bob", vec![example(9.0, 32)]);
        let query = example(1.0, 32);
        let matched = match_person(&query, &[alice, bob]).unwrap();
        assert_eq!(matched.person_id, "alice");
        assert!(matched.similarity > 0.99);
    }

    #[test]
    fn a_single_confirmed_example_is_enough_to_match() {
        // A person who was just named has exactly one confirmed face. Averaging
        // it with nothing else must not dilute the score below the threshold.
        let alice = PersonGallery::new("alice", "Alice", vec![example(1.0, 32)]);
        let query = example(1.0, 32);
        let matched = match_person(&query, &[alice]).unwrap();
        assert!((matched.similarity - 1.0).abs() < 1e-5);
        assert_eq!(matched.example_count, 1);
        assert_eq!(matched.consistent_examples, 1);
    }

    #[test]
    fn matching_counts_how_many_examples_agree() {
        // Three near-identical confirmed shots plus one unrelated: the close
        // ones must be reported as agreeing, not hidden behind an average.
        let mut examples = vec![example(9.0, 32)];
        for _ in 0..3 {
            examples.push(example(1.0, 32));
        }
        let alice = PersonGallery::new("alice", "Alice", examples);
        let matched = match_person(&example(1.0, 32), &[alice]).unwrap();
        assert!((matched.similarity - 1.0).abs() < 1e-5);
        assert_eq!(matched.consistent_examples, 3);
        assert_eq!(matched.example_count, 4);
    }

    #[test]
    fn empty_and_invalid_galleries_never_match() {
        let empty = PersonGallery::new("nobody", "Nobody", Vec::<Vec<f32>>::new());
        assert!(empty.is_empty());
        assert!(match_person(&example(1.0, 8), &[empty]).is_none());
        assert!(match_person(&[f32::NAN; 4], &[]).is_none());
        assert!(match_person(&[], &[]).is_none());
    }

    #[test]
    fn gallery_drops_unusable_examples() {
        let gallery = PersonGallery::new(
            "p",
            "P",
            vec![vec![0.0; 8], vec![f32::NAN; 8], example(3.0, 8)],
        );
        assert_eq!(gallery.examples.len(), 1);
        for value in &gallery.examples[0] {
            assert!(value.is_finite());
        }
    }

    #[test]
    fn candidate_threshold_is_inclusive() {
        let matched = PersonMatch {
            person_id: "p".into(),
            display_name: "P".into(),
            similarity: 0.363,
            consistent_examples: 1,
            example_count: 1,
        };
        assert!(matched.is_candidate(0.363));
        assert!(!matched.is_candidate(0.364));
    }

    #[test]
    fn ranked_similarities_are_sorted_descending() {
        let gallery = PersonGallery::new(
            "p",
            "P",
            vec![example(9.0, 16), example(1.0, 16), example(5.0, 16)],
        );
        let ranked = gallery.ranked_similarities(&example(1.0, 16));
        assert_eq!(ranked.len(), 3);
        assert!(ranked.windows(2).all(|pair| pair[0] >= pair[1]));
        assert!((ranked[0] - 1.0).abs() < 1e-5);
    }
}
