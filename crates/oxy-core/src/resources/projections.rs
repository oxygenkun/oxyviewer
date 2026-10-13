//! Rebuildable metadata and image projections.
//!
//! A projection is an accepted observation about a file: it can always be
//! recomputed from the source. Ratings and labels stored here mirror the user's
//! XMP sidecars, but the sidecar remains the source of truth, so clearing this
//! table is a re-read, not a loss.

use super::repo;
use crate::browse::types::AssetSummary;
use crate::resources::types::{ImageProjection, MetadataProjection, ResourceLoadStatus};
use crate::storage::Store;
use oxy_media::types::RenderLevel;
use rusqlite::TransactionBehavior;
use std::{path::Path, sync::Arc};

pub(crate) struct ResourceProjections {
    store: Arc<Store>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProjectionError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl ResourceProjections {
    pub(crate) fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    /// Allocates a revision in SQLite so observations from separate native
    /// processes share one comparable acceptance order.
    pub(crate) fn next_resource_revision(&self) -> Result<u64, ProjectionError> {
        let mut connection = self.store.write();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = repo::next_resource_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    pub(crate) fn metadata_projection(
        &self,
        path: &Path,
        source_revision: &str,
    ) -> Result<Option<MetadataProjection>, ProjectionError> {
        let connection = self.store.read_projection();
        let projection = repo::read_metadata_projection(&connection, path)?;
        Ok(projection.filter(|projection| projection.source_revision == source_revision))
    }

    /// Returns a stale-while-revalidate snapshot using only the source stat
    /// already present in an `AssetSummary`. Callers must still validate the
    /// complete revision, including sidecar or embedded metadata fingerprints.
    pub(crate) fn metadata_projection_for_asset(
        &self,
        asset: &AssetSummary,
    ) -> Result<Option<MetadataProjection>, ProjectionError> {
        let connection = self.store.read_projection();
        let prefix = format!("{}:{}:", asset.modified_at_ms, asset.size_bytes);
        Ok(
            repo::read_metadata_projection(&connection, &asset.path)?.filter(|projection| {
                projection.status == ResourceLoadStatus::Ready
                    && projection.source_revision.starts_with(&prefix)
            }),
        )
    }

    /// Atomically accepts a metadata observation or returns the newer state
    /// that already won. The returned revision is the only revision sent to UI.
    pub(crate) fn accept_metadata_projection(
        &self,
        mut candidate: MetadataProjection,
    ) -> Result<MetadataProjection, ProjectionError> {
        let mut connection = self.store.write();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(current) = repo::read_metadata_projection(&transaction, &candidate.path)?
            && current.valid_at > candidate.valid_at
        {
            transaction.commit()?;
            return Ok(current);
        }
        candidate.state_revision = repo::next_resource_revision(&transaction)?;
        repo::write_metadata(&transaction, &candidate)?;
        transaction.commit()?;
        Ok(candidate)
    }

    pub(crate) fn image_projection(
        &self,
        path: &Path,
        level: RenderLevel,
        source_revision: &str,
    ) -> Result<Option<ImageProjection>, ProjectionError> {
        let connection = self.store.read_projection();
        let projection = repo::read_image_projection(&connection, path, level)?;
        Ok(projection.filter(|projection| projection.source_revision == source_revision))
    }

    /// Atomically accepts an image artifact observation using the same
    /// transaction sequence as metadata projections.
    /// A nonzero state revision is an optimistic restore: it may replace a
    /// process-local descriptor only while that observed state is still current.
    pub(crate) fn accept_image_projection(
        &self,
        mut candidate: ImageProjection,
    ) -> Result<ImageProjection, ProjectionError> {
        let mut connection = self.store.write();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(current) =
            repo::read_image_projection(&transaction, &candidate.path, candidate.level)?
            && (current.valid_at > candidate.valid_at
                || (candidate.state_revision != 0
                    && current.state_revision > candidate.state_revision))
        {
            transaction.commit()?;
            return Ok(current);
        }
        candidate.state_revision = repo::next_resource_revision(&transaction)?;
        // Resource descriptors identify entries in the current process registry.
        // Persist only restart-safe artifact facts; a caller restoring a managed
        // file must register it again in its own process.
        let persisted_result = candidate.result.as_ref().and_then(|result| {
            let restart_safe = matches!(
                result.persistence,
                Some(oxy_media::types::MediaPersistence::Persisted)
                    | Some(oxy_media::types::MediaPersistence::NotApplicable)
                    | None
            ) && result.path.is_file();
            restart_safe.then(|| {
                let mut result = result.clone();
                result.resource = None;
                result
            })
        });
        let result_json = persisted_result
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        repo::write_image(&transaction, &candidate, result_json)?;
        transaction.commit()?;
        Ok(candidate)
    }

    pub(crate) fn invalidate_resource_projections(
        &self,
        directory: &Path,
    ) -> Result<(), ProjectionError> {
        let mut connection = self.store.write();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = repo::next_resource_revision(&transaction)?;
        repo::invalidate_directory(&transaction, revision, directory)?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn invalidate_image_projections(&self) -> Result<(), ProjectionError> {
        let mut connection = self.store.write();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = repo::next_resource_revision(&transaction)?;
        repo::invalidate_images(&transaction, revision)?;
        transaction.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browse::types::AssetSummary;
    use crate::resources::types::{ImageProjection, MetadataProjection, ResourceLoadStatus};
    use oxy_media::types::{AssetKind, PickLabel, RenderLevel};
    use rusqlite::params;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn projection_policy_delegates_statements_to_its_repository() {
        let source = include_str!("projections.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for verb in ["SELECT ", "INSERT INTO ", "UPDATE ", "DELETE FROM "] {
            assert!(
                !source.contains(verb),
                "projection SQL belongs in resources::repo: {verb}"
            );
        }
    }

    impl ResourceProjections {
        fn open(path: &Path) -> Result<Self, crate::storage::StoreError> {
            Ok(Self::new(Arc::new(Store::open(path)?)))
        }
        fn in_memory() -> Result<Self, crate::storage::StoreError> {
            Ok(Self::new(Arc::new(Store::in_memory()?)))
        }
    }
    #[test]
    fn metadata_projection_survives_database_reopen() {
        let state = tempdir().unwrap();
        let database = state.path().join("projections.sqlite");
        let path = state.path().join("rated.HIF");
        let source_revision = "100:200:xmp-a";
        {
            let projections = ResourceProjections::open(&database).unwrap();
            let valid_at = projections.next_resource_revision().unwrap();
            let accepted = projections
                .accept_metadata_projection(MetadataProjection {
                    path: path.clone(),
                    source_revision: source_revision.into(),
                    state_revision: 0,
                    valid_at,
                    status: ResourceLoadStatus::Ready,
                    rating: Some(4),
                    color_label: Some("Red".into()),
                    pick_label: Some(PickLabel::Accepted),
                    error: None,
                })
                .unwrap();
            assert!(accepted.state_revision > valid_at);
        }

        let reopened = ResourceProjections::open(&database).unwrap();
        let cached = reopened
            .metadata_projection(&path, source_revision)
            .unwrap()
            .unwrap();
        assert_eq!(cached.rating, Some(4));
        assert_eq!(cached.color_label.as_deref(), Some("Red"));
        assert_eq!(cached.pick_label, Some(PickLabel::Accepted));
    }

    #[test]
    fn cheap_asset_stat_can_publish_a_snapshot_before_full_revision_validation() {
        let projections = ResourceProjections::in_memory().unwrap();
        let path = PathBuf::from("C:/photos/rated.HIF");
        let asset = AssetSummary {
            id: "rated".into(),
            path: path.clone(),
            name: "rated.HIF".into(),
            extension: "HIF".into(),
            kind: AssetKind::Heif,
            size_bytes: 200,
            modified_at_ms: 100,
            has_sidecar: false,
            rating: None,
            color_label: None,
            pick_label: None,
        };
        let valid_at = projections.next_resource_revision().unwrap();
        projections
            .accept_metadata_projection(MetadataProjection {
                path,
                source_revision: "100:200:0:0:embedded-xmp-digest".into(),
                state_revision: 0,
                valid_at,
                status: ResourceLoadStatus::Ready,
                rating: Some(3),
                color_label: Some("Yellow".into()),
                pick_label: None,
                error: None,
            })
            .unwrap();

        assert_eq!(
            projections
                .metadata_projection_for_asset(&asset)
                .unwrap()
                .unwrap()
                .rating,
            Some(3)
        );
        assert!(
            projections
                .metadata_projection_for_asset(&AssetSummary {
                    modified_at_ms: 101,
                    ..asset
                })
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sqlite_order_rejects_an_older_result_from_another_connection() {
        let state = tempdir().unwrap();
        let database = state.path().join("projections.sqlite");
        let first = ResourceProjections::open(&database).unwrap();
        let second = ResourceProjections::open(&database).unwrap();
        let path = state.path().join("shared.HIF");
        let source_revision = "shared-source";
        let older = first.next_resource_revision().unwrap();
        let newer = second.next_resource_revision().unwrap();

        second
            .accept_metadata_projection(MetadataProjection {
                path: path.clone(),
                source_revision: source_revision.into(),
                state_revision: 0,
                valid_at: newer,
                status: ResourceLoadStatus::Ready,
                rating: Some(5),
                color_label: None,
                pick_label: None,
                error: None,
            })
            .unwrap();
        let winner = first
            .accept_metadata_projection(MetadataProjection {
                path: path.clone(),
                source_revision: source_revision.into(),
                state_revision: 0,
                valid_at: older,
                status: ResourceLoadStatus::Ready,
                rating: Some(1),
                color_label: None,
                pick_label: None,
                error: None,
            })
            .unwrap();

        assert_eq!(winner.valid_at, newer);
        assert_eq!(winner.rating, Some(5));
        assert_eq!(
            first
                .metadata_projection(&path, source_revision)
                .unwrap()
                .unwrap()
                .rating,
            Some(5)
        );
    }

    #[test]
    fn stale_descriptor_restore_cannot_overwrite_a_newer_state_of_the_same_request() {
        let projections = ResourceProjections::in_memory().unwrap();
        let loading = projections
            .accept_image_projection(ImageProjection {
                path: PathBuf::from("one.HIF"),
                source_revision: "source".into(),
                state_revision: 0,
                valid_at: projections.next_resource_revision().unwrap(),
                status: ResourceLoadStatus::Loading,
                level: RenderLevel::Thumbnail,
                result: None,
                error: None,
            })
            .unwrap();
        let terminal = projections
            .accept_image_projection(ImageProjection {
                state_revision: 0,
                status: ResourceLoadStatus::Error,
                error: Some("decode failed".into()),
                ..loading.clone()
            })
            .unwrap();
        let restored = projections.accept_image_projection(loading).unwrap();
        assert_eq!(restored.state_revision, terminal.state_revision);
        assert_eq!(restored.status, ResourceLoadStatus::Error);
        assert_eq!(restored.error, terminal.error);
    }

    #[test]
    fn image_projection_round_trips_artifact_without_pixel_payload() {
        let projections = ResourceProjections::in_memory().unwrap();
        let directory = tempdir().unwrap();
        let path = directory.path().join("one.HIF");
        let artifact = directory.path().join("one-preview.jpg");
        std::fs::write(&artifact, b"restart-safe artifact").unwrap();
        let valid_at = projections.next_resource_revision().unwrap();
        let accepted = projections
            .accept_image_projection(ImageProjection {
                path: path.clone(),
                source_revision: "image-source".into(),
                state_revision: 0,
                valid_at,
                status: ResourceLoadStatus::Ready,
                level: RenderLevel::Preview,
                result: Some(oxy_media::types::PreviewResult {
                    image_facts: None,
                    geometry: None,
                    path: artifact.clone(),
                    width: 1600,
                    height: 1067,
                    kind: oxy_media::types::PreviewKind::Decoded,
                    render_level: RenderLevel::Preview,
                    resource: None,
                    satisfaction: None,
                    persistence: None,
                    diagnostics: None,
                }),
                error: None,
            })
            .unwrap();

        assert!(accepted.state_revision > valid_at);
        let cached = projections
            .image_projection(&path, RenderLevel::Preview, "image-source")
            .unwrap()
            .unwrap();
        assert_eq!(cached.result.unwrap().path, artifact);
    }

    #[test]
    fn incompatible_image_projection_can_be_rebuilt_without_losing_ordering() {
        let projections = ResourceProjections::in_memory().unwrap();
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.HIF");
        let artifact = directory.path().join("thumbnail.jpg");
        std::fs::write(&artifact, b"restart-safe artifact").unwrap();
        let valid_at = projections.next_resource_revision().unwrap();
        let projection = ImageProjection {
            path: source.clone(),
            source_revision: "source-revision".into(),
            state_revision: 0,
            valid_at,
            status: ResourceLoadStatus::Ready,
            level: RenderLevel::Thumbnail,
            result: Some(oxy_media::types::PreviewResult {
                image_facts: None,
                geometry: None,
                path: artifact.clone(),
                width: 160,
                height: 120,
                kind: oxy_media::types::PreviewKind::Embedded,
                render_level: RenderLevel::Thumbnail,
                resource: None,
                satisfaction: Some(oxy_media::types::MediaSatisfaction::Interim),
                persistence: Some(oxy_media::types::MediaPersistence::Persisted),
                diagnostics: None,
            }),
            error: None,
        };
        let mut obsolete = serde_json::to_value(projection.result.as_ref().unwrap()).unwrap();
        // Reproduce the intermediate image-facts schema stored before sampledDimensions.
        obsolete["imageFacts"] = serde_json::json!({
            "exifOrientation": 1,
            "source": {
                "exifOrientation": 1, "revisionId": "source-revision",
                "candidateId": "sony-jpeg", "origin": "embeddedPreview",
                "encodedDimensions": {"width": 160, "height": 120},
                "displayDimensions": {"width": 160, "height": 120}
            },
            "encodedDimensions": {"width": 160, "height": 120},
            "displayDimensions": {"width": 160, "height": 120},
            "detail": {
                "referenceDimensions": {"width": 7008, "height": 4672},
                "region": {"x": 0, "y": 0, "width": 7008, "height": 4672},
                "sampling": "native"
            },
            "processing": [], "byteIntegrity": "sourcePayload"
        });
        assert!(
            serde_json::from_value::<oxy_media::types::PreviewResult>(obsolete.clone()).is_err()
        );
        for incompatible in [obsolete.to_string(), "{".into()] {
            let accepted = projections
                .accept_image_projection(projection.clone())
                .unwrap();
            projections
                .store
                .write()
                .execute(
                    "UPDATE resource_projections SET result_json = ?1 WHERE path = ?2",
                    params![incompatible, source.to_string_lossy()],
                )
                .unwrap();
            let cached = projections
                .image_projection(&source, RenderLevel::Thumbnail, "source-revision")
                .unwrap()
                .unwrap();
            assert!(cached.result.is_none());
            assert_eq!(cached.valid_at, accepted.valid_at);
            assert_eq!(cached.state_revision, accepted.state_revision);
            let mut stale = projection.clone();
            stale.valid_at = valid_at - 1;
            let rejected = projections.accept_image_projection(stale).unwrap();
            assert_eq!(rejected.state_revision, accepted.state_revision);
            assert!(rejected.result.is_none());
            let rebuilt = projections
                .accept_image_projection(projection.clone())
                .unwrap();
            assert!(rebuilt.state_revision > accepted.state_revision);
            let cached = projections
                .image_projection(&source, RenderLevel::Thumbnail, "source-revision")
                .unwrap()
                .unwrap();
            assert_eq!(cached.result.unwrap().path, artifact);
        }
    }

    #[test]
    fn image_projection_strips_process_local_resource_before_sqlite_persistence() {
        let projections = ResourceProjections::in_memory().unwrap();
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.HIF");
        let artifact = directory.path().join("artifact.jpg");
        std::fs::write(&artifact, b"restart-safe artifact").unwrap();
        let valid_at = projections.next_resource_revision().unwrap();
        let projection = ImageProjection {
            path: source.clone(),
            source_revision: "source-revision".into(),
            state_revision: 0,
            valid_at,
            status: ResourceLoadStatus::Ready,
            level: RenderLevel::Preview,
            result: Some(oxy_media::types::PreviewResult {
                image_facts: None,
                geometry: None,
                path: artifact,
                width: 16,
                height: 8,
                kind: oxy_media::types::PreviewKind::Decoded,
                render_level: RenderLevel::Preview,
                resource: Some(oxy_media::types::MediaResourceDescriptor {
                    resource_id: "old-process-resource".into(),
                    url: "oxy-media://localhost/resource/old-process-resource".into(),
                    media_type: "image/jpeg".into(),
                }),
                satisfaction: Some(oxy_media::types::MediaSatisfaction::Satisfied),
                persistence: Some(oxy_media::types::MediaPersistence::Persisted),
                diagnostics: None,
            }),
            error: None,
        };
        let accepted = projections.accept_image_projection(projection).unwrap();
        assert!(accepted.result.unwrap().resource.is_some());

        let restored = projections
            .image_projection(&source, RenderLevel::Preview, "source-revision")
            .unwrap()
            .unwrap();
        assert!(restored.result.unwrap().resource.is_none());
    }

    #[test]
    fn pending_resource_only_projection_is_not_restart_persisted() {
        let projections = ResourceProjections::in_memory().unwrap();
        let source = PathBuf::from("pending.HIF");
        let valid_at = projections.next_resource_revision().unwrap();
        projections
            .accept_image_projection(ImageProjection {
                path: source.clone(),
                source_revision: "source-revision".into(),
                state_revision: 0,
                valid_at,
                status: ResourceLoadStatus::Ready,
                level: RenderLevel::Preview,
                result: Some(oxy_media::types::PreviewResult {
                    image_facts: None,
                    geometry: None,
                    path: PathBuf::new(),
                    width: 16,
                    height: 8,
                    kind: oxy_media::types::PreviewKind::Embedded,
                    render_level: RenderLevel::Preview,
                    resource: Some(oxy_media::types::MediaResourceDescriptor {
                        resource_id: "pending-resource".into(),
                        url: "oxy-media://localhost/resource/pending-resource".into(),
                        media_type: "image/jpeg".into(),
                    }),
                    satisfaction: Some(oxy_media::types::MediaSatisfaction::Interim),
                    persistence: Some(oxy_media::types::MediaPersistence::Pending),
                    diagnostics: None,
                }),
                error: None,
            })
            .unwrap();

        assert!(
            projections
                .image_projection(&source, RenderLevel::Preview, "source-revision")
                .unwrap()
                .unwrap()
                .result
                .is_none()
        );
    }

    #[test]
    fn directory_invalidation_blocks_a_late_worker_result() {
        let projections = ResourceProjections::in_memory().unwrap();
        let directory = PathBuf::from("C:/photos");
        let path = directory.join("late.HIF");
        let valid_at = projections.next_resource_revision().unwrap();
        let loading = MetadataProjection {
            path: path.clone(),
            source_revision: "source-before-refresh".into(),
            state_revision: 0,
            valid_at,
            status: ResourceLoadStatus::Loading,
            rating: None,
            color_label: None,
            pick_label: None,
            error: None,
        };
        projections
            .accept_metadata_projection(loading.clone())
            .unwrap();
        projections
            .invalidate_resource_projections(&directory)
            .unwrap();

        let rejected = projections
            .accept_metadata_projection(MetadataProjection {
                status: ResourceLoadStatus::Ready,
                rating: Some(5),
                ..loading
            })
            .unwrap();

        assert_eq!(rejected.error.as_deref(), Some("invalidated"));
        assert!(
            projections
                .metadata_projection(&path, "source-before-refresh")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sqlite_order_also_rejects_an_older_image_result() {
        let state = tempdir().unwrap();
        let database = state.path().join("projections.sqlite");
        let first = ResourceProjections::open(&database).unwrap();
        let second = ResourceProjections::open(&database).unwrap();
        let path = state.path().join("shared.HIF");
        let older = first.next_resource_revision().unwrap();
        let newer = second.next_resource_revision().unwrap();
        let old_artifact = state.path().join("old.jpg");
        let new_artifact = state.path().join("new.jpg");
        std::fs::write(&old_artifact, b"old").unwrap();
        std::fs::write(&new_artifact, b"new").unwrap();
        let make_projection = |valid_at, artifact: &Path| ImageProjection {
            path: path.clone(),
            source_revision: "same-image-source".into(),
            state_revision: 0,
            valid_at,
            status: ResourceLoadStatus::Ready,
            level: RenderLevel::Thumbnail,
            result: Some(oxy_media::types::PreviewResult {
                image_facts: None,
                geometry: None,
                path: artifact.to_owned(),
                width: 160,
                height: 120,
                kind: oxy_media::types::PreviewKind::Embedded,
                render_level: RenderLevel::Thumbnail,
                resource: None,
                satisfaction: None,
                persistence: None,
                diagnostics: None,
            }),
            error: None,
        };

        second
            .accept_image_projection(make_projection(newer, &new_artifact))
            .unwrap();
        let winner = first
            .accept_image_projection(make_projection(older, &old_artifact))
            .unwrap();

        assert_eq!(winner.valid_at, newer);
        assert_eq!(winner.result.unwrap().path, new_artifact);
    }
}
