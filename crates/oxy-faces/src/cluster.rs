//! Unknown-face clustering.
//!
//! Clustering exists to organize faces the user has not identified yet. It is
//! deliberately *not* the person model: a cluster id is cache, and confirming a
//! cluster creates a person plus one decision per member, never a rename of the
//! cluster. A later run with a different embedding model is free to produce
//! completely different clusters.
//!
//! The implementation is exact single-link (connected components) over cosine
//! similarity. That is `O(n^2)` in the number of supplied faces, so callers
//! pass only the unknown subset; an approximate index is the planned follow-up
//! once a library has more unknown faces than this can serve.

use oxy_domain::{FaceCluster, FaceObservationId};

use crate::FaceError;
use crate::matcher::{normalize_embedding, similarity_normalized};

/// A face to cluster.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterInput {
    pub observation_id: FaceObservationId,
    pub embedding: Vec<f32>,
}

/// Upper bound on exact pairwise comparison. Above this the caller must narrow
/// the scope (for example to faces added since the last run) rather than
/// silently degrade into a partial result.
pub const MAX_EXACT_FACES: usize = 20_000;

/// Groups unknown faces into clusters of at least `min_size` members.
///
/// Members that are only transitively connected — reachable through the
/// cluster but not similar to any sibling — are reported as outliers so a
/// review UI can leave them unselected.
pub fn cluster_embeddings(
    items: &[ClusterInput],
    threshold: f32,
    min_size: usize,
) -> Result<Vec<FaceCluster>, FaceError> {
    if items.len() > MAX_EXACT_FACES {
        return Err(FaceError::ClusteringTooLarge {
            count: items.len(),
            limit: MAX_EXACT_FACES,
        });
    }
    if items.is_empty() {
        return Ok(Vec::new());
    }

    let normalized: Vec<Vec<f32>> = items
        .iter()
        .map(|item| normalize_embedding(&item.embedding))
        .collect();

    let mut union = UnionFind::new(items.len());
    for left in 0..items.len() {
        for right in left + 1..items.len() {
            if similarity_normalized(&normalized[left], &normalized[right]) >= threshold {
                union.join(left, right);
            }
        }
    }

    let mut groups: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for index in 0..items.len() {
        let root = union.find(index);
        groups.entry(root).or_default().push(index);
    }

    let min_size = min_size.max(2);
    let mut clusters = Vec::new();
    for mut members in groups.into_values() {
        if members.len() < min_size {
            continue;
        }
        // Deterministic ordering keeps cluster ids and representatives stable
        // for identical input, which makes re-running analysis idempotent.
        members.sort_by(|left, right| {
            items[*left]
                .observation_id
                .cmp(&items[*right].observation_id)
        });

        let mut similarity_sums = vec![0.0f64; members.len()];
        let mut cohesion = 0.0f64;
        let mut pairs = 0usize;
        for (i, left) in members.iter().enumerate() {
            for (j, right) in members.iter().enumerate().skip(i + 1) {
                let value = f64::from(similarity_normalized(
                    &normalized[*left],
                    &normalized[*right],
                ));
                similarity_sums[i] += value;
                similarity_sums[j] += value;
                cohesion += value;
                pairs += 1;
            }
        }
        let divisor = (members.len() - 1).max(1) as f64;
        let mean_similarity: Vec<f64> = similarity_sums.iter().map(|sum| sum / divisor).collect();
        let cluster_mean = mean_similarity.iter().sum::<f64>() / members.len() as f64;
        // Single link guarantees every member has at least one edge at or above
        // the threshold, so "below threshold" can never identify a boundary
        // member. A member is a boundary member when it sits noticeably below
        // the rest of its own cluster -- halfway between the cluster mean and
        // the connection threshold.
        let outlier_floor = cluster_mean - 0.5 * (cluster_mean - f64::from(threshold));

        let representative = members
            .iter()
            .enumerate()
            .max_by(|(left_index, _), (right_index, _)| {
                mean_similarity[*left_index]
                    .partial_cmp(&mean_similarity[*right_index])
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or(members[0], |(_, member)| *member);

        let outliers: Vec<FaceObservationId> = members
            .iter()
            .enumerate()
            .filter(|(index, _)| mean_similarity[*index] < outlier_floor)
            .map(|(_, member)| items[*member].observation_id.clone())
            .collect();

        let observation_ids: Vec<FaceObservationId> = members
            .iter()
            .map(|member| items[*member].observation_id.clone())
            .collect();
        clusters.push(FaceCluster {
            cluster_id: cluster_id(&observation_ids),
            representative_observation_id: items[representative].observation_id.clone(),
            member_count: members.len(),
            cohesion: if pairs == 0 {
                0.0
            } else {
                (cohesion / pairs as f64) as f32
            },
            outlier_observation_ids: outliers,
            observation_ids,
            suggested_person_id: None,
            suggested_name: None,
        });
    }

    // Largest clusters first, then by id, so the review queue is stable.
    clusters.sort_by(|left, right| {
        right
            .member_count
            .cmp(&left.member_count)
            .then_with(|| left.cluster_id.cmp(&right.cluster_id))
    });
    Ok(clusters)
}

/// Deterministic, content-derived cluster id. Two identical member sets produce
/// the same id, which keeps re-runs from looking like brand-new clusters.
fn cluster_id(observation_ids: &[FaceObservationId]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for id in observation_ids {
        hasher.update(id.as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    let hex: String = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("cluster-{hex}")
}

struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
            rank: vec![0; len],
        }
    }

    fn find(&mut self, mut index: usize) -> usize {
        while self.parent[index] != index {
            self.parent[index] = self.parent[self.parent[index]];
            index = self.parent[index];
        }
        index
    }

    fn join(&mut self, left: usize, right: usize) {
        let left = self.find(left);
        let right = self.find(right);
        if left == right {
            return;
        }
        match self.rank[left].cmp(&self.rank[right]) {
            std::cmp::Ordering::Less => self.parent[left] = right,
            std::cmp::Ordering::Greater => self.parent[right] = left,
            std::cmp::Ordering::Equal => {
                self.parent[right] = left;
                self.rank[left] += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::normalize_embedding;

    fn vector(seed: f32, dim: usize) -> Vec<f32> {
        normalize_embedding(
            &(0..dim)
                .map(|index| ((index as f32 + 1.0) * seed).sin())
                .collect::<Vec<f32>>(),
        )
    }

    fn close_to(seed: f32, dim: usize, noise: f32) -> Vec<f32> {
        let mut value = vector(seed, dim);
        for (index, entry) in value.iter_mut().enumerate() {
            *entry += noise * ((index as f32) * 0.7).cos();
        }
        normalize_embedding(&value)
    }

    fn inputs(ids: &[&str], embeddings: Vec<Vec<f32>>) -> Vec<ClusterInput> {
        ids.iter()
            .zip(embeddings)
            .map(|(id, embedding)| ClusterInput {
                observation_id: (*id).to_string(),
                embedding,
            })
            .collect()
    }

    #[test]
    fn separates_two_well_apart_groups() {
        let items = inputs(
            &["a1", "a2", "a3", "b1", "b2"],
            vec![
                close_to(1.0, 64, 0.01),
                close_to(1.0, 64, 0.02),
                close_to(1.0, 64, 0.015),
                close_to(9.0, 64, 0.01),
                close_to(9.0, 64, 0.02),
            ],
        );
        let clusters = cluster_embeddings(&items, 0.36, 2).unwrap();
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].member_count, 3);
        assert_eq!(clusters[1].member_count, 2);
        assert!(clusters[0].cohesion > 0.9);
    }

    #[test]
    fn singletons_are_not_clusters() {
        let items = inputs(
            &["a1", "b1", "c1"],
            vec![vector(1.0, 32), vector(5.0, 32), vector(9.0, 32)],
        );
        assert!(cluster_embeddings(&items, 0.36, 2).unwrap().is_empty());
    }

    #[test]
    fn min_size_is_at_least_two() {
        let items = inputs(
            &["a1", "a2"],
            vec![close_to(1.0, 32, 0.0), close_to(1.0, 32, 0.0)],
        );
        // A request for one-member clusters must not turn every face into one.
        let clusters = cluster_embeddings(&items, 0.36, 1).unwrap();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].member_count, 2);
    }

    #[test]
    fn boundary_members_are_reported_as_outliers() {
        // a, b, c are three tight near-identical shots. d is only weakly
        // connected: it clears the threshold against some of them but is far
        // from the rest of the cluster, so it must be flagged for the reviewer
        // rather than silently confirmed as the same person.
        let items = inputs(
            &["a", "b", "c", "d"],
            vec![
                normalize_embedding(&[1.0, 0.0]),
                normalize_embedding(&[0.98, 0.199]),
                normalize_embedding(&[0.98, -0.199]),
                normalize_embedding(&[0.45, 0.893]),
            ],
        );
        let clusters = cluster_embeddings(&items, 0.36, 2).unwrap();
        assert_eq!(clusters.len(), 1);
        let cluster = &clusters[0];
        assert_eq!(cluster.member_count, 4);
        assert_eq!(
            cluster.outlier_observation_ids,
            vec!["d".to_string()],
            "only the weakly connected member should be flagged"
        );
        assert_ne!(cluster.representative_observation_id, "d");
    }

    #[test]
    fn a_uniform_cluster_has_no_outliers() {
        let items = inputs(
            &["a1", "a2", "a3"],
            vec![
                normalize_embedding(&[1.0, 0.0]),
                normalize_embedding(&[0.99, 0.141]),
                normalize_embedding(&[0.99, -0.141]),
            ],
        );
        let clusters = cluster_embeddings(&items, 0.36, 2).unwrap();
        assert!(clusters[0].outlier_observation_ids.is_empty());
    }

    #[test]
    fn cluster_ids_are_deterministic_and_order_independent() {
        let forward = inputs(
            &["a1", "a2"],
            vec![close_to(1.0, 32, 0.0), close_to(1.0, 32, 0.001)],
        );
        let reversed = inputs(
            &["a2", "a1"],
            vec![close_to(1.0, 32, 0.001), close_to(1.0, 32, 0.0)],
        );
        let left = cluster_embeddings(&forward, 0.3, 2).unwrap();
        let right = cluster_embeddings(&reversed, 0.3, 2).unwrap();
        assert_eq!(left[0].cluster_id, right[0].cluster_id);
        assert_eq!(left[0].observation_ids, right[0].observation_ids);
    }

    #[test]
    fn empty_input_produces_no_clusters() {
        assert!(cluster_embeddings(&[], 0.3, 2).unwrap().is_empty());
    }

    #[test]
    fn oversized_input_is_rejected_instead_of_partially_clustered() {
        let items: Vec<ClusterInput> = (0..MAX_EXACT_FACES + 1)
            .map(|index| ClusterInput {
                observation_id: format!("f{index}"),
                embedding: vec![1.0, 0.0],
            })
            .collect();
        assert!(matches!(
            cluster_embeddings(&items, 0.3, 2),
            Err(FaceError::ClusteringTooLarge { .. })
        ));
    }
}
