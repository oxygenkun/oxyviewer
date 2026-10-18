//! Metadata read and edit use cases, with application-owned write workers.
use super::{
    MetadataFacade,
    provider::{ExiftoolStatus, ProviderManager},
    types::{AssetDetailsResult, EmbeddedSyncOptions, EmbeddedSyncResult, MetadataPatch},
};
use crate::{
    files::FsCatalog,
    jobs::types::{JobId, JobPriority},
    jobs::{JobRegistry, JobTicket},
    resources::{
        metadata::MetadataQueue,
        types::{MetadataProjection, MetadataRequestPriority},
    },
    runtime::CoreRuntime,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver},
    },
};

pub struct MetadataService {
    pub(crate) runtime: CoreRuntime,
    pub(crate) files: Arc<FsCatalog>,
    pub(crate) metadata: MetadataFacade,
    pub(crate) queue: MetadataQueue,
    pub(crate) jobs: Arc<JobRegistry>,
    pub(crate) provider: Arc<ProviderManager>,
    pub(crate) provider_mutation: Mutex<()>,
    pub(crate) embedded_mutation: Mutex<()>,
}

enum ProviderWork {
    Status,
    Configure(PathBuf),
    Install,
}

enum Edit {
    Patch(MetadataPatch),
}

struct EditingJob {
    registry: Arc<JobRegistry>,
    ticket: JobTicket,
}

impl Drop for EditingJob {
    fn drop(&mut self) {
        self.registry.finish(&self.ticket.id);
    }
}

impl MetadataService {
    pub fn provider_status(
        self: &Arc<Self>,
    ) -> Result<Receiver<Result<ExiftoolStatus, String>>, String> {
        self.start_provider(ProviderWork::Status)
    }

    pub fn configure_provider(
        self: &Arc<Self>,
        path: PathBuf,
    ) -> Result<Receiver<Result<ExiftoolStatus, String>>, String> {
        self.start_provider(ProviderWork::Configure(path))
    }

    pub fn install_provider(
        self: &Arc<Self>,
    ) -> Result<Receiver<Result<ExiftoolStatus, String>>, String> {
        self.start_provider(ProviderWork::Install)
    }

    fn start_provider(
        self: &Arc<Self>,
        work: ProviderWork,
    ) -> Result<Receiver<Result<ExiftoolStatus, String>>, String> {
        let service = Arc::clone(self);
        let (sender, receiver) = mpsc::channel();
        self.runtime
            .spawn_named("metadata-provider".into(), move || {
                // Configuration and installation share one destination. Status
                // remains independent, so observing it does not wait for download.
                let _mutation = if matches!(&work, ProviderWork::Status) {
                    None
                } else {
                    Some(
                        service
                            .provider_mutation
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner),
                    )
                };
                let result = if service.runtime.is_stopping() {
                    Err("application is shutting down".into())
                } else {
                    match work {
                        ProviderWork::Status => Ok(service.provider.status()),
                        ProviderWork::Configure(path) => service.provider.set_user_executable(path),
                        ProviderWork::Install => service.provider.install_managed(),
                    }
                };
                let _ = sender.send(result);
            })
            .map_err(|error| error.to_string())?;
        Ok(receiver)
    }

    pub fn details(&self, path: PathBuf) -> Result<AssetDetailsResult, String> {
        self.queue
            .request_details(&self.runtime, path)?
            .recv()
            .map_err(|error| format!("metadata queue stopped: {error}"))
    }

    pub fn request(
        &self,
        paths: Vec<PathBuf>,
        priority: MetadataRequestPriority,
    ) -> Result<Vec<MetadataProjection>, String> {
        self.queue.request_summaries(&self.runtime, paths, priority)
    }

    pub fn patch(
        self: &Arc<Self>,
        paths: Vec<PathBuf>,
        patch: MetadataPatch,
    ) -> Result<Receiver<Result<JobId, String>>, String> {
        self.start_edit(paths, Edit::Patch(patch))
    }

    pub fn sync_to_embedded(
        self: &Arc<Self>,
        paths: Vec<PathBuf>,
        request_id: String,
    ) -> Result<Receiver<Result<EmbeddedSyncResult, String>>, String> {
        self.sync_to_embedded_with_options(paths, request_id, EmbeddedSyncOptions::default())
    }

    /// Explicit limits support controlled qualification on a particular volume.
    /// Desktop callers use the bounded default through `sync_to_embedded`.
    pub fn sync_to_embedded_with_options(
        self: &Arc<Self>,
        paths: Vec<PathBuf>,
        request_id: String,
        options: EmbeddedSyncOptions,
    ) -> Result<Receiver<Result<EmbeddedSyncResult, String>>, String> {
        if !(1..=4).contains(&options.concurrency) {
            return Err("embedded metadata concurrency must be between 1 and 4".into());
        }
        let job = EditingJob {
            registry: Arc::clone(&self.jobs),
            ticket: self.jobs.register(JobPriority::SelectedMetadata),
        };
        let service = Arc::clone(self);
        let (sender, receiver) = mpsc::channel();
        self.runtime
            .spawn_named("metadata-embedded-batch".into(), move || {
                let result = service.run_embedded_batch(paths, request_id, &job.ticket, options);
                drop(job);
                let _ = sender.send(Ok(result));
            })
            .map_err(|error| error.to_string())?;
        Ok(receiver)
    }

    fn start_edit(
        self: &Arc<Self>,
        paths: Vec<PathBuf>,
        edit: Edit,
    ) -> Result<Receiver<Result<JobId, String>>, String> {
        let job = EditingJob {
            registry: self.jobs.clone(),
            ticket: self.jobs.register(JobPriority::SelectedMetadata),
        };
        let service = self.clone();
        let (sender, receiver) = mpsc::channel();
        self.runtime
            .spawn_named("metadata-edit".into(), move || {
                let result = service
                    .run_edit(paths, edit, &job.ticket)
                    .map(|()| job.ticket.id.clone());
                drop(job);
                let _ = sender.send(result);
            })
            .map_err(|error| error.to_string())?;
        Ok(receiver)
    }

    fn run_edit(&self, paths: Vec<PathBuf>, edit: Edit, ticket: &JobTicket) -> Result<(), String> {
        let mut changed = Vec::new();
        let result = (|| {
            for path in paths {
                if self.runtime.is_stopping() || ticket.is_cancelled() {
                    return Err("metadata edit cancelled".to_owned());
                }
                let asset = self
                    .files
                    .get_asset(&path)
                    .map_err(|error| error.to_string())?;
                match &edit {
                    Edit::Patch(patch) => {
                        self.metadata
                            .patch_metadata(&asset.path, asset.kind, patch)
                            .map_err(|error| error.to_string())?;
                    }
                }
                // The file write has already succeeded. Invalidate it even if
                // a later member of this batch fails or shutdown intervenes.
                if let Some(parent) = asset.path.parent() {
                    self.files.invalidate_directory(parent);
                }
                self.queue.invalidate_path(&asset.path);
                changed.push(asset.path);
            }
            Ok(())
        })();
        let refresh = if changed.is_empty() || self.runtime.is_stopping() {
            Ok(())
        } else {
            self.request(changed, MetadataRequestPriority::Selected)
                .map(|_| ())
        };
        result.and(refresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppPaths, OxyApp, events::CoreEvent};
    use oxy_media::types::AssetKind;
    use std::time::{Duration, Instant};

    #[test]
    fn embedded_batch_deduplicates_continues_errors_and_skips_equal_without_a_provider() {
        let directory = tempfile::tempdir().unwrap();
        let (app, events) = OxyApp::open(AppPaths::from_roots(
            directory.path().join("state"),
            directory.path().join("cache"),
        ))
        .unwrap();
        let mut photos = Vec::new();
        for name in ["a.jpg", "b.jpg"] {
            let path = directory.path().join(name);
            image::RgbImage::new(8, 8).save(&path).unwrap();
            app.metadata_service
                .metadata
                .patch_metadata(&path, AssetKind::Jpeg, &MetadataPatch::default())
                .unwrap();
            photos.push(path);
        }
        app.metadata_service
            .metadata
            .set_exiftool(Some(directory.path().join("missing-provider")));
        let raw = directory.path().join("raw.arw");
        std::fs::write(&raw, b"raw").unwrap();
        let result = app
            .metadata_service
            .sync_to_embedded(
                vec![
                    photos[0].clone(),
                    directory.path().join("missing.jpg"),
                    photos[1].clone(),
                    photos[0].clone(),
                    raw,
                ],
                "request".into(),
            )
            .unwrap()
            .recv()
            .unwrap()
            .unwrap();
        assert_eq!(result.total, 4);
        assert_eq!(result.completed, 4);
        assert_eq!(result.updated, 0);
        assert_eq!(result.unchanged, 2);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.failures.len(), 1);
        assert!(!result.failures[0].uncertain);
        assert!(!app.jobs.cancel(&result.job_id));
        let progress = events
            .try_iter()
            .filter_map(|event| match event {
                CoreEvent::EmbeddedSyncProgress(value) => Some(value),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(progress.len(), 5);
        assert!(
            progress
                .iter()
                .all(|event| event.request_id == "request" && event.job_id == result.job_id)
        );
        assert_eq!(progress.last().unwrap().completed, 4);
        assert!(app.shutdown(Duration::from_secs(3)));
        assert!(
            app.metadata_service
                .sync_to_embedded(photos, "after-stop".into())
                .is_err()
        );
    }

    #[test]
    fn waiting_embedded_batch_cancels_and_remains_owned_until_its_worker_exits() {
        let directory = tempfile::tempdir().unwrap();
        let photo = directory.path().join("a.jpg");
        image::RgbImage::new(8, 8).save(&photo).unwrap();
        let before = std::fs::read(&photo).unwrap();
        let (app, events) = OxyApp::open(AppPaths::from_roots(
            directory.path().join("state"),
            directory.path().join("cache"),
        ))
        .unwrap();
        let guard = app.metadata_service.embedded_mutation.lock().unwrap();
        let receiver = app
            .metadata_service
            .sync_to_embedded(vec![photo.clone()], "cancel".into())
            .unwrap();
        let job_id = loop {
            if let CoreEvent::EmbeddedSyncProgress(progress) =
                events.recv_timeout(Duration::from_secs(3)).unwrap()
            {
                break progress.job_id;
            }
        };
        assert!(app.jobs.cancel(&job_id));
        assert!(!app.shutdown(Duration::ZERO));
        drop(guard);
        let result = receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        assert!(result.cancelled);
        assert_eq!(result.completed, 0);
        assert_eq!(std::fs::read(photo).unwrap(), before);
        assert!(app.shutdown(Duration::from_secs(3)));
    }

    #[test]
    fn editing_one_path_preserves_other_cached_details() {
        let directory = tempfile::tempdir().unwrap();
        let a = directory.path().join("a.jpg");
        let b = directory.path().join("b.jpg");
        image::RgbImage::new(8, 8).save(&a).unwrap();
        image::RgbImage::new(8, 8).save(&b).unwrap();
        let (app, _) = OxyApp::open(AppPaths::from_roots(
            directory.path().join("state"),
            directory.path().join("cache"),
        ))
        .unwrap();
        app.metadata_service.details(a.clone()).unwrap();
        let before = app.metadata_service.details(b.clone()).unwrap();
        app.metadata_service
            .patch(
                vec![a.clone()],
                MetadataPatch {
                    rating: Some(Some(4)),
                    ..MetadataPatch::default()
                },
            )
            .unwrap()
            .recv()
            .unwrap()
            .unwrap();
        let after = app.metadata_service.details(b).unwrap();
        assert_eq!(
            before.metadata_projection.state_revision,
            after.metadata_projection.state_revision
        );
        assert_eq!(
            app.metadata_service
                .details(a)
                .unwrap()
                .details
                .metadata
                .rating,
            Some(4)
        );
        assert!(app.shutdown(Duration::from_secs(3)));
    }

    #[cfg(unix)]
    #[test]
    fn active_exiftool_is_not_reported_joined_at_the_shutdown_deadline() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let photo = directory.path().join("a.jpg");
        image::RgbImage::new(8, 8).save(&photo).unwrap();
        let (app, _) = OxyApp::open(AppPaths::from_roots(
            directory.path().join("state"),
            directory.path().join("cache"),
        ))
        .unwrap();
        app.metadata_service
            .metadata
            .patch_metadata(
                &photo,
                AssetKind::Jpeg,
                &MetadataPatch {
                    rating: Some(Some(4)),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        let executable = directory.path().join("fake-exiftool");
        let entered = directory.path().join("entered");
        let release = directory.path().join("release");
        std::fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
while IFS= read -r arg; do
  case "$arg" in
    -stay_open) IFS= read -r flag; [ "$flag" = False ] && exit 0 ;;
    -execute*)
      id=${{arg#-execute}}
      touch '{}'
      while [ ! -f '{}' ]; do sleep 0.01; done
      printf 'oxy-status-%s:0\n' "$id" >&2
      printf '{{ready%s}}\n' "$id"
      ;;
  esac
done
"#,
                entered.display(),
                release.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        app.metadata_service.metadata.set_exiftool(Some(executable));
        let receiver = app
            .metadata_service
            .sync_to_embedded(vec![photo], "active".into())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !entered.is_file() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.shutdown(Duration::ZERO));
        std::fs::write(release, b"finish").unwrap();
        assert!(
            receiver
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap()
                .cancelled
        );
        assert!(app.shutdown(Duration::from_secs(3)));
    }

    #[test]
    fn provider_work_is_owned_and_waiting_install_cannot_start_after_shutdown() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        let (app, _) = OxyApp::open(AppPaths {
            data_dir: data.clone(),
            preview_dir: directory.path().join("cache"),
        })
        .unwrap();
        let guard = app.metadata_service.provider_mutation.lock().unwrap();
        let result = app.metadata_service.install_provider().unwrap();
        assert!(
            !app.shutdown(std::time::Duration::ZERO),
            "live worker was reported joined"
        );
        drop(guard);
        assert!(app.shutdown(std::time::Duration::from_secs(3)));
        assert!(
            result
                .recv()
                .unwrap()
                .unwrap_err()
                .contains("shutting down")
        );
        assert!(!data.join("providers").exists());
        assert!(app.metadata_service.install_provider().is_err());
        assert!(
            app.metadata_service
                .configure_provider(data.join("tool"))
                .is_err()
        );
        assert!(app.metadata_service.provider_status().is_err());
    }

    #[test]
    fn partial_batch_refreshes_written_metadata_and_shutdown_rejects_new_edits() {
        let directory = tempfile::tempdir().unwrap();
        let photo = directory.path().join("a.jpg");
        image::RgbImage::new(8, 8).save(&photo).unwrap();
        let photo = photo.canonicalize().unwrap();
        let (app, _) = OxyApp::open(AppPaths {
            data_dir: directory.path().join("data"),
            preview_dir: directory.path().join("cache"),
        })
        .unwrap();
        let before = app.metadata_service.details(photo.clone()).unwrap();
        assert_ne!(before.details.metadata.rating, Some(4));
        let result = app
            .metadata_service
            .patch(
                vec![photo.clone(), directory.path().join("missing.jpg")],
                MetadataPatch {
                    rating: Some(Some(4)),
                    ..MetadataPatch::default()
                },
            )
            .unwrap()
            .recv()
            .unwrap();
        assert!(result.is_err());
        let after = app.metadata_service.details(photo.clone()).unwrap();
        assert_eq!(after.details.metadata.rating, Some(4));
        assert!(
            !app.jobs.cancel("job-0"),
            "finished job remained registered"
        );
        assert!(app.shutdown(std::time::Duration::from_secs(3)));
        assert!(
            app.metadata_service
                .patch(
                    vec![photo.clone()],
                    MetadataPatch {
                        rating: Some(Some(1)),
                        ..MetadataPatch::default()
                    }
                )
                .is_err()
        );
        assert_eq!(super::super::read_sidecar(&photo).unwrap().rating, Some(4));
        assert!(
            !app.jobs.cancel("job-1"),
            "rejected job remained registered"
        );
    }
}
