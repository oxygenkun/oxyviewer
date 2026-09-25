//! Rebuildable person feature cache. Manual identity facts never depend on it.

use crate::{Library, LibraryError};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::{collections::HashMap, path::PathBuf, sync::OnceLock};

static EXTENSION_REGISTRATION: OnceLock<Result<(), String>> = OnceLock::new();

pub(crate) fn register_extension() -> Result<(), String> {
    EXTENSION_REGISTRATION
        .get_or_init(|| {
            // The sqlite-vec crate exposes its C init symbol as an opaque function.
            // SQLite calls it with the standard extension-init ABI after registration.
            let init = unsafe {
                std::mem::transmute::<
                    *const (),
                    unsafe extern "C" fn(
                        *mut rusqlite::ffi::sqlite3,
                        *mut *mut std::ffi::c_char,
                        *const rusqlite::ffi::sqlite3_api_routines,
                    ) -> std::ffi::c_int,
                >(sqlite_vec::sqlite3_vec_init as *const ())
            };
            let code = unsafe { rusqlite::ffi::sqlite3_auto_extension(Some(init)) };
            if code == rusqlite::ffi::SQLITE_OK {
                Ok(())
            } else {
                Err(format!("SQLite extension registration failed: {code}"))
            }
        })
        .clone()
}

pub(crate) fn probe_connections(
    writer: &Connection,
    reader: Option<&parking_lot::Mutex<Connection>>,
    projection_reader: Option<&parking_lot::Mutex<Connection>>,
) -> Result<String, String> {
    let version = |connection: &Connection| {
        connection
            .query_row("SELECT vec_version() AS version", [], |row| {
                row.get::<_, String>("version")
            })
            .map_err(|error| error.to_string())
    };
    let expected = version(writer)?;
    for connection in [reader, projection_reader].into_iter().flatten() {
        let actual = version(&connection.lock())?;
        if actual != expected {
            return Err(format!(
                "sqlite-vec version mismatch: {expected} vs {actual}"
            ));
        }
    }
    Ok(expected)
}

const CACHE_SCHEMA_VERSION: i64 = 3;

oxy_store::table::tables! {
    preserve person_vector_cache_meta = "schema_version INTEGER NOT NULL";
    clear person_feature_spaces =
        "id TEXT PRIMARY KEY,
        modality TEXT NOT NULL CHECK(modality IN ('face','body')),
        dimension INTEGER NOT NULL CHECK(dimension > 0 AND dimension <= 4096),
        producer_fingerprint TEXT NOT NULL,
        format_version INTEGER NOT NULL DEFAULT 1";
    clear person_features_cache =
        "feature_row_id INTEGER PRIMARY KEY,
        folder_path TEXT NOT NULL,
        asset_path TEXT NOT NULL,
        instance_id TEXT NOT NULL,
        source_revision TEXT NOT NULL,
        feature_space_id TEXT NOT NULL REFERENCES person_feature_spaces(id),
        pipeline_fingerprint TEXT NOT NULL,
        vector BLOB NOT NULL,
        updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
        UNIQUE(folder_path,asset_path,instance_id,feature_space_id)";
}

pub(crate) fn ensure_schema(connection: &mut Connection) -> Result<(), rusqlite::Error> {
    let transaction = connection.transaction()?;
    oxy_store::table::create_all(&transaction, DEFS)?;
    let current: Option<i64> = transaction
        .query_row(
            "SELECT schema_version FROM person_vector_cache_meta LIMIT 1",
            [],
            |row| row.get("schema_version"),
        )
        .optional()?;
    if current != Some(CACHE_SCHEMA_VERSION) {
        transaction.execute_batch(
            "DROP TABLE IF EXISTS person_features_cache;
             DROP TABLE IF EXISTS person_feature_spaces;
             DELETE FROM person_vector_cache_meta;",
        )?;
        transaction.execute(
            "INSERT INTO person_vector_cache_meta(schema_version) VALUES (?1)",
            [CACHE_SCHEMA_VERSION],
        )?;
    }
    // Runs again because a version mismatch dropped the tables above.
    oxy_store::table::create_all(&transaction, DEFS)?;
    transaction.execute_batch(
        "CREATE INDEX IF NOT EXISTS person_features_folder_space
           ON person_features_cache(folder_path,feature_space_id,asset_path,instance_id);",
    )?;
    transaction.commit()
}

/// Empties the tables declared above.
///
/// Feature spaces go with the vectors they describe. The schema marker
/// survives; see [`crate::schema::preserved_on_clear`].
pub(crate) fn clear(connection: &Connection) -> Result<(), rusqlite::Error> {
    oxy_store::table::clear_all(connection, DEFS)
}

/// Points cached features at a renamed file. Called from [`crate::user`] when
/// a photo moves inside one folder, where the cache is still valid.
pub(crate) fn rename_asset(
    transaction: &Transaction<'_>,
    source: &str,
    destination: &str,
) -> Result<(), rusqlite::Error> {
    transaction.execute(
        "UPDATE person_features_cache SET asset_path=?2 WHERE asset_path=?1",
        params![source, destination],
    )?;
    Ok(())
}

/// Drops cached features for a file that no longer exists at that path. Called
/// from [`crate::user`] when a photo is deleted or moved out of its folder.
pub(crate) fn forget_asset(
    transaction: &Transaction<'_>,
    path: &str,
) -> Result<(), rusqlite::Error> {
    transaction.execute(
        "DELETE FROM person_features_cache WHERE asset_path=?1",
        params![path],
    )?;
    Ok(())
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureModality {
    Face,
    Body,
}

impl FeatureModality {
    fn as_str(self) -> &'static str {
        match self {
            Self::Face => "face",
            Self::Body => "body",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersonFeatureMatch {
    pub feature_row_id: i64,
    pub asset_path: PathBuf,
    pub instance_id: String,
    pub source_revision: String,
    pub pipeline_fingerprint: String,
    pub similarity: f32,
}

pub struct PersonFeatureSearch<'a> {
    pub folder_path: &'a std::path::Path,
    pub feature_space_id: &'a str,
    pub query: &'a [f32],
    pub min_similarity: f32,
    pub page: (usize, usize),
    pub current_sources: &'a [(PathBuf, String)],
}

fn validate_vector(values: &[f32]) -> Result<(), LibraryError> {
    if values.is_empty() || values.len() > 4096 || values.iter().any(|value| !value.is_finite()) {
        return Err(LibraryError::InvalidPersonFeature);
    }
    let norm = values
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    if (norm - 1.0).abs() > 0.001 {
        return Err(LibraryError::InvalidPersonFeature);
    }
    Ok(())
}

pub(crate) fn validate_feature(feature: &PersonFeature) -> Result<(), LibraryError> {
    validate_vector(&feature.values)?;
    if feature.instance_id.is_empty()
        || feature.source_revision.is_empty()
        || feature.feature_space_id.is_empty()
        || feature.producer_fingerprint.is_empty()
        || feature.pipeline_fingerprint.is_empty()
        || feature.asset_path.parent() != Some(feature.folder_path.as_path())
    {
        return Err(LibraryError::InvalidPersonFeature);
    }
    Ok(())
}

fn vector_blob(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

pub(crate) fn write_feature(
    tx: &Transaction<'_>,
    feature: &PersonFeature,
) -> Result<(), LibraryError> {
    tx.execute(
        "INSERT OR IGNORE INTO person_feature_spaces(id,modality,dimension,producer_fingerprint)
         VALUES (?1,?2,?3,?4)",
        params![
            feature.feature_space_id,
            feature.modality.as_str(),
            feature.values.len(),
            feature.producer_fingerprint
        ],
    )?;
    let contract: (String, i64, String) = tx.query_row(
        "SELECT modality,dimension,producer_fingerprint FROM person_feature_spaces WHERE id=?1",
        [&feature.feature_space_id],
        |row| {
            Ok((
                row.get("modality")?,
                row.get("dimension")?,
                row.get("producer_fingerprint")?,
            ))
        },
    )?;
    if contract
        != (
            feature.modality.as_str().to_owned(),
            feature.values.len() as i64,
            feature.producer_fingerprint.clone(),
        )
    {
        return Err(LibraryError::InvalidPersonFeature);
    }
    tx.execute(
        "INSERT INTO person_features_cache(folder_path,asset_path,instance_id,source_revision,feature_space_id,pipeline_fingerprint,vector)
         VALUES (?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(folder_path,asset_path,instance_id,feature_space_id) DO UPDATE SET
           source_revision=excluded.source_revision,pipeline_fingerprint=excluded.pipeline_fingerprint,
           vector=excluded.vector,updated_at=unixepoch()",
        params![feature.folder_path.to_string_lossy(),feature.asset_path.to_string_lossy(),
            feature.instance_id,feature.source_revision,feature.feature_space_id,
            feature.pipeline_fingerprint,vector_blob(&feature.values)],
    )?;
    Ok(())
}

impl Library {
    pub fn person_vector_status(&self) -> Result<&str, &str> {
        self.vector_status.as_deref().map_err(String::as_str)
    }

    pub fn put_person_feature(&self, feature: &PersonFeature) -> Result<(), LibraryError> {
        self.vector_status
            .as_ref()
            .map_err(|error| LibraryError::PersonVectorUnavailable(error.clone()))?;
        validate_feature(feature)?;
        let mut connection = self.write();
        let tx = connection.transaction()?;
        write_feature(&tx, feature)?;
        tx.commit()?;
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
    ) -> Result<Vec<PersonFeatureMatch>, LibraryError> {
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
    ) -> Result<Vec<PersonFeatureMatch>, LibraryError> {
        if detection_producer_fingerprint.len() != 64
            || !detection_producer_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(LibraryError::InvalidPersonFeature);
        }
        self.search_person_features_impl(search, Some(detection_producer_fingerprint))
    }

    fn search_person_features_impl(
        &self,
        search: PersonFeatureSearch<'_>,
        detection_producer_fingerprint: Option<&str>,
    ) -> Result<Vec<PersonFeatureMatch>, LibraryError> {
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
            .map_err(|error| LibraryError::PersonVectorUnavailable(error.clone()))?;
        validate_vector(query)?;
        if !min_similarity.is_finite()
            || !(-1.0..=1.0).contains(&min_similarity)
            || limit == 0
            || limit > 1000
        {
            return Err(LibraryError::InvalidPersonFeature);
        }
        let mut source_by_path = HashMap::with_capacity(current_sources.len());
        for (path, revision) in current_sources {
            if path.parent() != Some(folder_path)
                || revision.is_empty()
                || source_by_path
                    .insert(path.as_path(), revision.as_str())
                    .is_some()
            {
                return Err(LibraryError::InvalidPersonFeature);
            }
        }
        if source_by_path.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.read_connection();
        let dimension: Option<i64> = connection
            .query_row(
                "SELECT dimension FROM person_feature_spaces WHERE id=?1",
                [feature_space_id],
                |row| row.get("dimension"),
            )
            .optional()?;
        let Some(dimension) = dimension else {
            return Ok(Vec::new());
        };
        if dimension != query.len() as i64 {
            return Err(LibraryError::InvalidPersonFeature);
        }
        let vector = vector_blob(query);
        let mut statement = connection.prepare(
            "SELECT feature_row_id,asset_path,instance_id,source_revision,pipeline_fingerprint,
               1.0-vec_distance_cosine(vector,?1) AS similarity
             FROM person_features_cache AS f WHERE folder_path=?2 AND feature_space_id=?3
               AND 1.0-vec_distance_cosine(vector,?1)>=?4
               AND (?5 IS NULL OR EXISTS (
                 SELECT 1 FROM person_instances_cache AS d
                  WHERE d.folder_path=f.folder_path AND d.asset_path=f.asset_path
                    AND d.instance_id=f.instance_id AND d.source_revision=f.source_revision
                    AND d.producer_fingerprint=?5))
             ORDER BY similarity DESC,asset_path,instance_id,feature_row_id",
        )?;
        let rows = statement.query_map(
            params![
                vector,
                folder_path.to_string_lossy(),
                feature_space_id,
                min_similarity,
                detection_producer_fingerprint
            ],
            |row| {
                Ok(PersonFeatureMatch {
                    feature_row_id: row.get("feature_row_id")?,
                    asset_path: row.get::<_, String>("asset_path")?.into(),
                    instance_id: row.get("instance_id")?,
                    source_revision: row.get("source_revision")?,
                    pipeline_fingerprint: row.get("pipeline_fingerprint")?,
                    similarity: row.get("similarity")?,
                })
            },
        )?;
        let mut matches = Vec::with_capacity(limit);
        let mut eligible_seen = 0usize;
        for row in rows {
            let candidate = row?;
            if source_by_path.get(candidate.asset_path.as_path()).copied()
                != Some(candidate.source_revision.as_str())
            {
                continue;
            }
            if eligible_seen >= offset {
                matches.push(candidate);
                if matches.len() == limit {
                    break;
                }
            }
            eligible_seen += 1;
        }
        Ok(matches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::detections::{DetectionStageContext, replace_stage_detections};
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
        let library = Library::in_memory().unwrap();
        assert!(library.person_vector_status().is_ok());
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
            library.put_person_feature(item).unwrap();
        }
        let body = feature(
            "body",
            "d.jpg",
            "body-v1",
            FeatureModality::Body,
            [1.0, 0.0, 0.0],
        );
        library.put_person_feature(&body).unwrap();
        let sources = current(&[&original, &near, &unrelated, &body]);
        let first_page = library
            .search_person_features(
                Path::new("/photos"),
                "face-v1",
                &[1.0, 0.0, 0.0],
                0.7,
                (1, 0),
                &sources,
            )
            .unwrap();
        let second_page = library
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
        library.put_person_feature(&same_producer).unwrap();
        let changed_pipeline = library
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
            library.put_person_feature(&changed_producer),
            Err(LibraryError::InvalidPersonFeature)
        ));
        let mut changed_sources = sources.clone();
        changed_sources[0].1 = "new-source".into();
        let eligible_first = library
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
            library
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
            library
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
            library
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
            library.put_person_feature(&feature(
                "bad",
                "e.jpg",
                "face-v1",
                FeatureModality::Body,
                [1.0, 0.0, 0.0]
            )),
            Err(LibraryError::InvalidPersonFeature)
        ));
        assert!(matches!(
            library.put_person_feature(&feature(
                "bad",
                "e.jpg",
                "face-v1",
                FeatureModality::Face,
                [f32::NAN, 0.0, 0.0]
            )),
            Err(LibraryError::InvalidPersonFeature)
        ));
        assert!(matches!(
            library.search_person_features(
                Path::new("/photos"),
                "face-v1",
                &[1.0, 0.0],
                0.0,
                (10, 0),
                &sources
            ),
            Err(LibraryError::InvalidPersonFeature)
        ));
    }

    #[test]
    fn instance_cache_ids_are_scoped_to_the_asset() {
        let library = Library::in_memory().unwrap();
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
        library.put_person_feature(&first).unwrap();
        library.put_person_feature(&second).unwrap();
        let result = library
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
        let library = Library::in_memory().unwrap();
        let producer = "b".repeat(64);
        let mut cached = feature(
            &"a".repeat(64),
            "a.jpg",
            "face-v1",
            FeatureModality::Face,
            [1.0, 0.0, 0.0],
        );
        cached.source_revision = "c".repeat(64);
        library.put_person_feature(&cached).unwrap();
        let sources = current(&[&cached]);
        let search = || {
            library
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
            let mut connection = library.write();
            let transaction = connection.transaction().unwrap();
            replace_stage_detections(&transaction, &context, &[detection]).unwrap();
            transaction.commit().unwrap();
        }
        assert_eq!(search().len(), 1);
        {
            let mut connection = library.write();
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
        let library = Library::open(&database).unwrap();
        let first = feature(
            "instance",
            "a.jpg",
            "face-v1",
            FeatureModality::Face,
            [1.0, 0.0, 0.0],
        );
        library.put_person_feature(&first).unwrap();
        drop(library);
        let reopened = Library::open(&database).unwrap();
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
        reopened
            .move_asset_tag_state(&replacement.asset_path, &moved)
            .unwrap();
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
        let library = Library::open(&database).unwrap();
        let person = library
            .create_folder_person(Path::new("/photos"), "manual")
            .unwrap();
        let cached = feature(
            "instance",
            "a.jpg",
            "face-v1",
            FeatureModality::Face,
            [1.0, 0.0, 0.0],
        );
        library.put_person_feature(&cached).unwrap();
        library
            .write()
            .execute("UPDATE person_vector_cache_meta SET schema_version=0", [])
            .unwrap();
        drop(library);
        let reopened = Library::open(&database).unwrap();
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
        let mut library = Library::in_memory().unwrap();
        library.vector_status = Err("test unavailable".into());
        let person = library
            .create_folder_person(Path::new("/photos"), "manual")
            .unwrap();
        assert_eq!(
            library.list_folder_people(Path::new("/photos")).unwrap()[0].id,
            person.id
        );
        assert!(matches!(
            library.put_person_feature(&feature(
                "one",
                "a.jpg",
                "face-v1",
                FeatureModality::Face,
                [1.0, 0.0, 0.0]
            )),
            Err(LibraryError::PersonVectorUnavailable(_))
        ));
    }
}
