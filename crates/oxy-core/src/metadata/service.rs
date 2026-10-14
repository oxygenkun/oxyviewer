//! Metadata read and edit use cases, with application-owned write workers.
use super::{
    MetadataFacade,
    provider::{ExiftoolStatus, ProviderManager},
    types::{AssetDetailsResult, MetadataPatch},
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
use oxy_media::types::AssetKind;
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
}

enum ProviderWork {
    Status,
    Configure(PathBuf),
    Install,
}

enum Edit {
    Patch(MetadataPatch),
    Embedded,
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
    ) -> Result<Receiver<Result<JobId, String>>, String> {
        self.start_edit(paths, Edit::Embedded)
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
                    Edit::Embedded if asset.kind == AssetKind::Raw => continue,
                    Edit::Embedded => {
                        self.metadata
                            .sync_metadata_to_embedded(&asset.path)
                            .map_err(|error| error.to_string())?;
                    }
                }
                // The file write has already succeeded. Invalidate it even if
                // a later member of this batch fails or shutdown intervenes.
                if let Some(parent) = asset.path.parent() {
                    self.files.invalidate_directory(parent);
                    self.queue.invalidate_directory(parent);
                }
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
    use crate::{AppPaths, OxyApp};

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
