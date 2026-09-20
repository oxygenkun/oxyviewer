//! Unknown-face clustering.
//!
//! Clustering exists to organize faces the user has not identified yet. It is
//! deliberately *not* the person model: a cluster id is cache, and confirming a
//! cluster creates a person plus one decision per member, never a rename of the
//! cluster. A later run with a different embedding model is free to produce
//! completely different clusters.
//!
//! The implementation builds an exact reciprocal-kNN graph, then considers its
//! edges from strongest to weakest. Components merge only when their average
//! cross-similarity still clears the configured threshold. This prevents one
//! borderline face from bridging two otherwise distinct identities, which is
//! the main failure mode of thresholded single-link clustering.
//!
//! The graph is still `O(n^2)` in the number of supplied faces, so callers pass
//! only the unknown subset; an approximate index is the planned follow-up once
//! a library has more unknown faces than this can serve.

use oxy_domain::{FaceCluster, FaceObservationId};

use crate::FaceError;
use crate::matcher::{normalize_embedding, similarity_normalized};

/// A face to cluster.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterInput {
    pub observation_id: FaceObservationId,
    /// Faces observed in the same asset are a hard cannot-link constraint.
    /// `None` is retained for callers that only have embeddings.
    pub asset_id: Option<String>,
    pub embedding: Vec<f32>,
}

/// Upper bound on exact pairwise comparison. Above this the caller must narrow
/// the scope (for example to faces added since the last run) rather than
/// silently degrade into a partial result.
pub const MAX_EXACT_FACES: usize = 20_000;

/// A small neighborhood is intentional: an edge should describe local identity
/// structure, not merely be above a global threshold somewhere in the library.
const RECIPROCAL_NEIGHBORS: usize = 12;

#[derive(Debug, Clone, Copy)]
struct Neighbor {
    index: usize,
    similarity: f32,
}

#[derive(Debug, Clone, Copy)]
struct Edge {
    left: usize,
    right: usize,
    similarity: f32,
}

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
    cluster_embeddings_cancellable(items, threshold, min_size, || false)
}

/// Exact clustering with cooperative cancellation in both quadratic passes.
pub fn cluster_embeddings_cancellable(
    items: &[ClusterInput],
    threshold: f32,
    min_size: usize,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<FaceCluster>, FaceError> {
    crate::check_cancelled(&cancelled)?;
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
        .map(|item| {
            crate::check_cancelled(&cancelled)?;
            Ok(normalize_embedding(&item.embedding))
        })
        .collect::<Result<_, FaceError>>()?;

    let mut neighborhoods = vec![Vec::new(); items.len()];
    for left in 0..items.len() {
        crate::check_cancelled(&cancelled)?;
        for right in left + 1..items.len() {
            if (right - left) % 1024 == 1 {
                crate::check_cancelled(&cancelled)?;
            }
            if same_known_asset(&items[left], &items[right]) {
                continue;
            }
            let similarity = similarity_normalized(&normalized[left], &normalized[right]);
            if similarity >= threshold {
                insert_neighbor(&mut neighborhoods[left], right, similarity);
                insert_neighbor(&mut neighborhoods[right], left, similarity);
            }
        }
    }

    let mut edges = Vec::new();
    for (left, neighbors) in neighborhoods.iter().enumerate() {
        crate::check_cancelled(&cancelled)?;
        for neighbor in neighbors {
            let right = neighbor.index;
            if left < right
                && neighborhoods[right]
                    .iter()
                    .any(|candidate| candidate.index == left)
            {
                edges.push(Edge {
                    left,
                    right,
                    similarity: neighbor.similarity,
                });
            }
        }
    }
    edges.sort_by(|left, right| {
        right
            .similarity
            .partial_cmp(&left.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                items[left.left]
                    .observation_id
                    .cmp(&items[right.left].observation_id)
            })
            .then_with(|| {
                items[left.right]
                    .observation_id
                    .cmp(&items[right.right].observation_id)
            })
    });

    let mut union = UnionFind::new(items.len());
    let mut component_members: Vec<Vec<usize>> =
        (0..items.len()).map(|index| vec![index]).collect();
    for edge in edges {
        crate::check_cancelled(&cancelled)?;
        let left_root = union.find(edge.left);
        let right_root = union.find(edge.right);
        if left_root == right_root {
            continue;
        }
        if components_have_asset_conflict(
            &component_members[left_root],
            &component_members[right_root],
            items,
        ) {
            continue;
        }
        let (cross_sum, cross_pairs) = cross_similarity(
            &component_members[left_root],
            &component_members[right_root],
            &normalized,
            &cancelled,
        )?;
        if cross_sum / cross_pairs as f64 + f64::EPSILON < f64::from(threshold) {
            continue;
        }
        let (root, absorbed) = union.join(left_root, right_root);
        if root != absorbed {
            let moved = std::mem::take(&mut component_members[absorbed]);
            component_members[root].extend(moved);
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
        crate::check_cancelled(&cancelled)?;
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
            crate::check_cancelled(&cancelled)?;
            for (j, right) in members.iter().enumerate().skip(i + 1) {
                if (j - i) % 1024 == 1 {
                    crate::check_cancelled(&cancelled)?;
                }
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
        // Every non-singleton component was assembled from accepted graph
        // edges, so "below threshold" alone is not a useful boundary signal.
        // A member is a boundary member when it sits noticeably below the rest
        // of its own cluster -- halfway between the cluster mean and the merge
        // threshold.
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
            member_quality: Vec::new(),
        });
    }

    crate::check_cancelled(&cancelled)?;
    // Largest clusters first, then by id, so the review queue is stable.
    clusters.sort_by(|left, right| {
        right
            .member_count
            .cmp(&left.member_count)
            .then_with(|| left.cluster_id.cmp(&right.cluster_id))
    });
    Ok(clusters)
}

fn same_known_asset(left: &ClusterInput, right: &ClusterInput) -> bool {
    left.asset_id
        .as_ref()
        .zip(right.asset_id.as_ref())
        .is_some_and(|(left, right)| left == right)
}

fn components_have_asset_conflict(left: &[usize], right: &[usize], items: &[ClusterInput]) -> bool {
    left.iter().any(|left_index| {
        right
            .iter()
            .any(|right_index| same_known_asset(&items[*left_index], &items[*right_index]))
    })
}

fn insert_neighbor(neighbors: &mut Vec<Neighbor>, index: usize, similarity: f32) {
    let position = neighbors
        .binary_search_by(|neighbor| {
            neighbor
                .similarity
                .partial_cmp(&similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
                .reverse()
                .then_with(|| neighbor.index.cmp(&index))
        })
        .unwrap_or_else(|position| position);
    neighbors.insert(position, Neighbor { index, similarity });
    neighbors.truncate(RECIPROCAL_NEIGHBORS);
}

fn cross_similarity(
    left: &[usize],
    right: &[usize],
    normalized: &[Vec<f32>],
    cancelled: &impl Fn() -> bool,
) -> Result<(f64, usize), FaceError> {
    let mut sum = 0.0;
    let mut pairs = 0;
    for left_index in left {
        crate::check_cancelled(cancelled)?;
        for right_index in right {
            sum += f64::from(similarity_normalized(
                &normalized[*left_index],
                &normalized[*right_index],
            ));
            pairs += 1;
        }
    }
    Ok((sum, pairs))
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

    fn join(&mut self, left: usize, right: usize) -> (usize, usize) {
        let left = self.find(left);
        let right = self.find(right);
        if left == right {
            return (left, right);
        }
        match self.rank[left].cmp(&self.rank[right]) {
            std::cmp::Ordering::Less => {
                self.parent[left] = right;
                (right, left)
            }
            std::cmp::Ordering::Greater => {
                self.parent[right] = left;
                (left, right)
            }
            std::cmp::Ordering::Equal => {
                self.parent[right] = left;
                self.rank[left] += 1;
                (left, right)
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
                asset_id: None,
                embedding,
            })
            .collect()
    }

    #[test]
    fn cancellation_interrupts_pairwise_and_cohesion_passes() {
        let count = 64;
        let items: Vec<_> = (0..count)
            .map(|index| ClusterInput {
                observation_id: format!("face-{index}"),
                asset_id: None,
                embedding: vec![1.0, 0.0],
            })
            .collect();
        for cancel_at in [count + 4, 3 * count + 4] {
            let checks = std::cell::Cell::new(0);
            let result = cluster_embeddings_cancellable(&items, 0.5, 2, || {
                checks.set(checks.get() + 1);
                checks.get() >= cancel_at
            });
            assert!(matches!(result, Err(FaceError::Cancelled)));
        }
    }

    #[test]
    fn a_borderline_bridge_does_not_merge_two_dense_identities() {
        let angle = |degrees: f32| {
            let radians = degrees.to_radians();
            vec![radians.cos(), radians.sin()]
        };
        let items = inputs(
            &["a1", "a2", "b1", "b2"],
            vec![angle(0.0), angle(10.0), angle(45.0), angle(55.0)],
        );

        let clusters = cluster_embeddings(&items, 35.0f32.to_radians().cos(), 2).unwrap();

        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].observation_ids, vec!["a1", "a2"]);
        assert_eq!(clusters[1].observation_ids, vec!["b1", "b2"]);
    }

    #[test]
    fn same_asset_faces_remain_separate_at_component_level() {
        let mut items = inputs(
            &["a1", "a2", "b1", "b2"],
            vec![
                normalize_embedding(&[1.0, 0.0]),
                normalize_embedding(&[0.99, 0.03]),
                normalize_embedding(&[0.98, 0.08]),
                normalize_embedding(&[0.97, 0.10]),
            ],
        );
        items[0].asset_id = Some("photo-1".into());
        items[1].asset_id = Some("photo-2".into());
        items[2].asset_id = Some("photo-1".into());
        items[3].asset_id = Some("photo-2".into());

        let clusters = cluster_embeddings(&items, 0.9, 2).unwrap();

        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].observation_ids, vec!["a1", "a2"]);
        assert_eq!(clusters[1].observation_ids, vec!["b1", "b2"]);
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
                asset_id: None,
                embedding: vec![1.0, 0.0],
            })
            .collect();
        assert!(matches!(
            cluster_embeddings(&items, 0.3, 2),
            Err(FaceError::ClusteringTooLarge { .. })
        ));
    }
}
