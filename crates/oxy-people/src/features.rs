//! Rebuildable person feature cache. Manual identity facts never depend on it.
//!
//! The tables, their format marker, the sqlite-vec registration, and the
//! statements that score, write, or follow a rename live in `oxy_store`; what
//! stays here is what a feature *means* — how a vector is validated against the
//! feature space it claims to belong to, what a rebuild may replace, and what
//! makes a cached vector still eligible for a candidate query.

use crate::{People, PeopleError};
use oxy_domain::{FeatureModality, PersonFeatureMatch};
use oxy_store::{StoreError, Transaction, repo};
use std::{collections::HashMap, path::PathBuf};

#[derive(Debug, Clone)]
pub struct PersonFeature {
    pub folder_path: PathBuf,
    pub asset_path: PathBuf,
    pub instance_id: String,
    pub source_revision: String,
    pub feature_space_id: String,
    pub modality: FeatureModality,
    pub producer_fingerprint: String,
    pub pipeline_fingerprint: String,
    pub values: Vec<f32>,
}

pub struct PersonFeatureSearch<'a> {
    pub folder_path: &'a std::path::Path,
    pub feature_space_id: &'a str,
    pub query: &'a [f32],
    pub min_similarity: f32,
    pub page: (usize, usize),
    pub current_sources: &'a [(PathBuf, String)],
}

fn validate_vector(values: &[f32]) -> Result<(), PeopleError> {
    if values.is_empty() || values.len() > 4096 || values.iter().any(|value| !value.is_finite()) {
        return Err(PeopleError::InvalidPersonFeature);
    }
    let norm = values
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    if (norm - 1.0).abs() > 0.001 {
        return Err(PeopleError::InvalidPersonFeature);
    }
    Ok(())
}

pub(crate) fn validate_feature(feature: &PersonFeature) -> Result<(), PeopleError> {
    validate_vector(&feature.values)?;
    if feature.instance_id.is_empty()
        || feature.source_revision.is_empty()
        || feature.feature_space_id.is_empty()
        || feature.producer_fingerprint.is_empty()
        || feature.pipeline_fingerprint.is_empty()
        || feature.asset_path.parent() != Some(feature.folder_path.as_path())
    {
        return Err(PeopleError::InvalidPersonFeature);
    }
    Ok(())
}

fn vector_blob(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Stores one vector after checking it against the feature space it claims.
///
/// A space belongs to the first producer that claimed it: the same id reused
/// with another dimension or another producer is not a compatible vector, so it
/// is rejected rather than silently overwritten.
pub(crate) fn write_feature(
    transaction: &Transaction<'_>,
    feature: &PersonFeature,
) -> Result<(), PeopleError> {
    repo::person_cache::ensure_feature_space(
        transaction,
        &feature.feature_space_id,
        feature.modality,
        feature.values.len(),
        &feature.producer_fingerprint,
    )?;
    let contract =
        repo::person_cache::feature_space_contract(transaction, &feature.feature_space_id)?;
    let Some((stored_modality, stored_dimension, stored_producer)) = contract else {
        return Err(PeopleError::InvalidPersonFeature);
    };
    if stored_modality != feature.modality.as_str()
        || stored_dimension != feature.values.len() as i64
        || stored_producer != feature.producer_fingerprint
    {
        return Err(PeopleError::InvalidPersonFeature);
    }
    repo::person_cache::upsert_feature(
        transaction,
        &repo::person_cache::NewFeature {
            folder_path: &feature.folder_path.to_string_lossy(),
            asset_path: &feature.asset_path.to_string_lossy(),
            instance_id: &feature.instance_id,
            source_revision: &feature.source_revision,
            feature_space_id: &feature.feature_space_id,
            pipeline_fingerprint: &feature.pipeline_fingerprint,
            vector: &vector_blob(&feature.values),
        },
    )?;
    Ok(())
}

impl People {
    pub fn person_vector_status(&self) -> Result<&str, &str> {
        self.vector_status.as_deref().map_err(String::as_str)
    }

    pub fn put_person_feature(&self, feature: &PersonFeature) -> Result<(), PeopleError> {
        self.vector_status
            .as_ref()
            .map_err(|error| PeopleError::PersonVectorUnavailable(error.clone()))?;
        validate_feature(feature)?;
        let mut connection = self.store.write();
        let transaction = connection.transaction().map_err(StoreError::from)?;
        write_feature(&transaction, feature)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(())
    }

    /// `current_sources` must describe the full current folder snapshot. Stale rows
    /// are filtered before paging, while all cached rows are scored for the threshold.
    /// A future vec0 index must preserve these exhaustive threshold semantics.
    pub fn search_person_features(
        &self,
        folder_path: &std::path::Path,
        feature_space_id: &str,
        query: &[f32],
        min_similarity: f32,
        page: (usize, usize),
        current_sources: &[(PathBuf, String)],
    ) -> Result<Vec<PersonFeatureMatch>, PeopleError> {
        self.search_person_features_impl(
            PersonFeatureSearch {
                folder_path,
                feature_space_id,
                query,
                min_similarity,
                page,
                current_sources,
            },
            None,
        )
    }

    /// Product candidate search: only current automatic detections from this
    /// producer may supply a feature. Detection reruns can leave old vectors in
    /// the rebuildable feature cache, so source checks alone are insufficient.
    pub fn search_current_detected_person_features(
        &self,
        search: PersonFeatureSearch<'_>,
        detection_producer_fingerprint: &str,
    ) -> Result<Vec<PersonFeatureMatch>, PeopleError> {
        if detection_producer_fingerprint.len() != 64
            || !detection_producer_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(PeopleError::InvalidPersonFeature);
        }
        self.search_person_features_impl(search, Some(detection_producer_fingerprint))
    }

    fn search_person_features_impl(
        &self,
        search: PersonFeatureSearch<'_>,
        detection_producer_fingerprint: Option<&str>,
    ) -> Result<Vec<PersonFeatureMatch>, PeopleError> {
        let PersonFeatureSearch {
            folder_path,
            feature_space_id,
            query,
            min_similarity,
            page,
            current_sources,
        } = search;
        let (limit, offset) = page;
        self.vector_status
            .as_ref()
            .map_err(|error| PeopleError::PersonVectorUnavailable(error.clone()))?;
        validate_vector(query)?;
        if !min_similarity.is_finite()
            || !(-1.0..=1.0).contains(&min_similarity)
            || limit == 0
            || limit > 1000
        {
            return Err(PeopleError::InvalidPersonFeature);
        }
        let mut source_by_path = HashMap::with_capacity(current_sources.len());
        for (path, revision) in current_sources {
            if path.parent() != Some(folder_path)
                || revision.is_empty()
                || source_by_path
                    .insert(path.as_path(), revision.as_str())
                    .is_some()
            {
                return Err(PeopleError::InvalidPersonFeature);
            }
        }
        if source_by_path.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.store.read();
        let dimension = repo::person_cache::feature_space_dimension(&connection, feature_space_id)?;
        let Some(dimension) = dimension else {
            return Ok(Vec::new());
        };
        if dimension != query.len() as i64 {
            return Err(PeopleError::InvalidPersonFeature);
        }
        let folder = folder_path.to_string_lossy();
        let current: Vec<(String, String)> = current_sources
            .iter()
            .map(|(path, revision)| (path.to_string_lossy().into_owned(), revision.clone()))
            .collect();
        Ok(repo::person_cache::search_features(
            &connection,
            &repo::person_cache::FeatureQuery {
                folder_path: &folder,
                feature_space_id,
                vector: &vector_blob(query),
                min_similarity,
                detection_producer_fingerprint,
                current_sources: &current,
                page: (limit, offset),
            },
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detections::{DetectionStageContext, replace_stage_detections};
    use crate::testing;
    use oxy_domain::DetectedPersonInstance;
    use std::path::Path;

    fn feature(
        instance: &str,
        image: &str,
        space: &str,
        modality: FeatureModality,
        values: [f32; 3],
    ) -> PersonFeature {
        PersonFeature {
            folder_path: PathBuf::from("/photos"),
            asset_path: PathBuf::from(format!("/photos/{image}")),
            instance_id: instance.into(),
            source_revision: "10:20".into(),
            feature_space_id: space.into(),
            modality,
            producer_fingerprint: "face-stage-v1".into(),
            pipeline_fingerprint: "pipeline-v1".into(),
            values: values.to_vec(),
        }
    }

    fn current(items: &[&PersonFeature]) -> Vec<(PathBuf, String)> {
        items
            .iter()
            .map(|item| (item.asset_path.clone(), item.source_revision.clone()))
            .collect()
    }

    #[test]
    fn vector_search_is_exhaustive_scoped_and_stably_paged() {
        let people = testing::in_memory();
        assert!(people.person_vector_status().is_ok());
        let original = feature(
            "first",
            "a.jpg",
            "face-v1",
            FeatureModality::Face,
            [1.0, 0.0, 0.0],
        );
        let near = feature(
            "second",
            "b.jpg",
            "face-v1",
            FeatureModality::Face,
            [0.8, 0.6, 0.0],
        );
        let unrelated = feature(
            "third",
            "c.jpg",
            "face-v1",
            FeatureModality::Face,
            [0.0, 1.0, 0.0],
        );
        for item in [&original, &near, &unrelated] {
            people.put_person_feature(item).unwrap();
        }
        let body = feature(
            "body",
            "d.jpg",
            "body-v1",
            FeatureModality::Body,
            [1.0, 0.0, 0.0],
        );
        people.put_person_feature(&body).unwrap();
        let sources = current(&[&original, &near, &unrelated, &body]);
        let first_page = people
            .search_person_features(
                Path::new("/photos"),
                "face-v1",
                &[1.0, 0.0, 0.0],
                0.7,
                (1, 0),
                &sources,
            )
            .unwrap();
        let second_page = people
            .search_person_features(
                Path::new("/photos"),
                "face-v1",
                &[1.0, 0.0, 0.0],
                0.7,
                (1, 1),
                &sources,
            )
            .unwrap();
        assert_eq!(first_page[0].instance_id, "first");
        assert_eq!(second_page[0].instance_id, "second");
        let mut same_producer = original.clone();
        same_producer.pipeline_fingerprint = "pipeline-with-new-body-v2".into();
        people.put_person_feature(&same_producer).unwrap();
        let changed_pipeline = people
            .search_person_features(
                Path::new("/photos"),
                "face-v1",
                &[1.0, 0.0, 0.0],
                0.99,
                (1, 0),
                &sources,
            )
            .unwrap();
        assert_eq!(
            changed_pipeline[0].pipeline_fingerprint,
            "pipeline-with-new-body-v2"
        );
        let mut changed_producer = original;
        changed_producer.producer_fingerprint = "face-stage-v2".into();
        assert!(matches!(
            people.put_person_feature(&changed_producer),
            Err(PeopleError::InvalidPersonFeature)
        ));
        let mut changed_sources = sources.clone();
        changed_sources[0].1 = "new-source".into();
        let eligible_first = people
            .search_person_features(
                Path::new("/photos"),
                "face-v1",
                &[1.0, 0.0, 0.0],
                0.7,
                (1, 0),
                &changed_sources,
            )
            .unwrap();
        assert_eq!(eligible_first[0].instance_id, "second");
        assert!(
            people
                .search_person_features(
                    Path::new("/photos"),
                    "face-v1",
                    &[1.0, 0.0, 0.0],
                    0.7,
                    (1, 1),
                    &changed_sources,
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            people
                .search_person_features(
                    Path::new("/other"),
                    "face-v1",
                    &[1.0, 0.0, 0.0],
                    -1.0,
                    (100, 0),
                    &[]
                )
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            people
                .search_person_features(
                    Path::new("/photos"),
                    "face-v1",
                    &[1.0, 0.0, 0.0],
                    -1.0,
                    (100, 0),
                    &sources
                )
                .unwrap()
                .len(),
            3
        );
        assert!(matches!(
            people.put_person_feature(&feature(
                "bad",
                "e.jpg",
                "face-v1",
                FeatureModality::Body,
                [1.0, 0.0, 0.0]
            )),
            Err(PeopleError::InvalidPersonFeature)
        ));
        assert!(matches!(
            people.put_person_feature(&feature(
                "bad",
                "e.jpg",
                "face-v1",
                FeatureModality::Face,
                [f32::NAN, 0.0, 0.0]
            )),
            Err(PeopleError::InvalidPersonFeature)
        ));
        assert!(matches!(
            people.search_person_features(
                Path::new("/photos"),
                "face-v1",
                &[1.0, 0.0],
                0.0,
                (10, 0),
                &sources
            ),
            Err(PeopleError::InvalidPersonFeature)
        ));
    }

    #[test]
    fn instance_cache_ids_are_scoped_to_the_asset() {
        let people = testing::in_memory();
        let first = feature(
            "face-0",
            "a.jpg",
            "face-v1",
            FeatureModality::Face,
            [1.0, 0.0, 0.0],
        );
        let second = feature(
            "face-0",
            "b.jpg",
            "face-v1",
            FeatureModality::Face,
            [0.0, 1.0, 0.0],
        );
        people.put_person_feature(&first).unwrap();
        people.put_person_feature(&second).unwrap();
        let result = people
            .search_person_features(
                Path::new("/photos"),
                "face-v1",
                &[1.0, 0.0, 0.0],
                -1.0,
                (10, 0),
                &current(&[&first, &second]),
            )
            .unwrap();
        assert_eq!(result.len(), 2);
        assert_ne!(result[0].asset_path, result[1].asset_path);
    }

    #[test]
    fn product_search_excludes_vectors_after_detection_is_removed() {
        let people = testing::in_memory();
        let producer = "b".repeat(64);
        let mut cached = feature(
            &"a".repeat(64),
            "a.jpg",
            "face-v1",
            FeatureModality::Face,
            [1.0, 0.0, 0.0],
        );
        cached.source_revision = "c".repeat(64);
        people.put_person_feature(&cached).unwrap();
        let sources = current(&[&cached]);
        let search = || {
            people
                .search_current_detected_person_features(
                    PersonFeatureSearch {
                        folder_path: Path::new("/photos"),
                        feature_space_id: "face-v1",
                        query: &[1.0, 0.0, 0.0],
                        min_similarity: 0.9,
                        page: (10, 0),
                        current_sources: &sources,
                    },
                    &producer,
                )
                .unwrap()
        };
        assert!(search().is_empty());
        let detection = DetectedPersonInstance {
            instance_id: cached.instance_id.clone(),
            face_box: Some([0.1, 0.1, 0.2, 0.2]),
            face_landmarks: None,
            body_box: None,
            face_score: Some(0.95),
            body_score: None,
            association_score: None,
        };
        let context = DetectionStageContext {
            folder: &cached.folder_path,
            asset: &cached.asset_path,
            source_revision: &cached.source_revision,
            producer_fingerprint: &producer,
            pipeline_fingerprint: &cached.pipeline_fingerprint,
            run_id: "run-1",
        };
        {
            let mut connection = people.store.write();
            let transaction = connection.transaction().unwrap();
            replace_stage_detections(&transaction, &context, &[detection]).unwrap();
            transaction.commit().unwrap();
        }
        assert_eq!(search().len(), 1);
        {
            let mut connection = people.store.write();
            let transaction = connection.transaction().unwrap();
            replace_stage_detections(&transaction, &context, &[]).unwrap();
            transaction.commit().unwrap();
        }
        assert!(search().is_empty());
    }

    #[test]
    fn feature_cache_survives_reopen_and_replaces_a_new_source_revision() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let people = testing::open(&database);
        let first = feature(
            "instance",
            "a.jpg",
            "face-v1",
            FeatureModality::Face,
            [1.0, 0.0, 0.0],
        );
        people.put_person_feature(&first).unwrap();
        drop(people);
        let reopened = testing::open(&database);
        let mut replacement = first;
        replacement.source_revision = "11:21".into();
        replacement.values = vec![0.0, 1.0, 0.0];
        reopened.put_person_feature(&replacement).unwrap();
        let results = reopened
            .search_person_features(
                Path::new("/photos"),
                "face-v1",
                &[0.0, 1.0, 0.0],
                0.99,
                (10, 0),
                &current(&[&replacement]),
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_revision, "11:21");
        let moved = PathBuf::from("/photos/renamed.jpg");
        {
            // The same-folder-move rule lives with the relocation act, which
            // spans the tag rows, the person rows, and this cache.
            let mut connection = reopened.store.write();
            let transaction = connection.transaction().unwrap();
            oxy_store::repo::cross::relocate_asset(
                &transaction,
                &replacement.asset_path.to_string_lossy(),
                &moved.to_string_lossy(),
                true,
            )
            .unwrap();
            transaction.commit().unwrap();
        }
        let moved_results = reopened
            .search_person_features(
                Path::new("/photos"),
                "face-v1",
                &[0.0, 1.0, 0.0],
                0.99,
                (10, 0),
                &[(moved.clone(), replacement.source_revision)],
            )
            .unwrap();
        assert_eq!(moved_results[0].asset_path, moved);
    }

    #[test]
    fn incompatible_vector_cache_rebuild_preserves_manual_people() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("library.sqlite");
        let people = testing::open(&database);
        let person = people
            .create_folder_person(Path::new("/photos"), "manual")
            .unwrap();
        let cached = feature(
            "instance",
            "a.jpg",
            "face-v1",
            FeatureModality::Face,
            [1.0, 0.0, 0.0],
        );
        people.put_person_feature(&cached).unwrap();
        people
            .store
            .write()
            .execute("UPDATE person_vector_cache_meta SET schema_version=0", [])
            .unwrap();
        drop(people);
        let reopened = testing::open(&database);
        assert_eq!(
            reopened.list_folder_people(Path::new("/photos")).unwrap()[0].id,
            person.id
        );
        assert!(
            reopened
                .search_person_features(
                    Path::new("/photos"),
                    "face-v1",
                    &[1.0, 0.0, 0.0],
                    0.0,
                    (10, 0),
                    &current(&[&cached]),
                )
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unavailable_extension_does_not_block_manual_people() {
        let mut people = testing::in_memory();
        people.vector_status = Err("test unavailable".into());
        let person = people
            .create_folder_person(Path::new("/photos"), "manual")
            .unwrap();
        assert_eq!(
            people.list_folder_people(Path::new("/photos")).unwrap()[0].id,
            person.id
        );
        assert!(matches!(
            people.put_person_feature(&feature(
                "one",
                "a.jpg",
                "face-v1",
                FeatureModality::Face,
                [1.0, 0.0, 0.0]
            )),
            Err(PeopleError::PersonVectorUnavailable(_))
        ));
    }
}
