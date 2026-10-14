//! Explicit file operations and durable recovery after partial failure.
use super::{
    FsCatalog, repo,
    types::{FileOperation, FileOperationRecovery, FileOperationResult},
};
use crate::{
    app::workflows::file_action, resources::metadata::MetadataQueue, runtime::CoreRuntime,
    storage::Store, tags::xmp_sync::TagXmpSync,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc::Receiver},
};

pub struct FileService {
    pub(crate) store: Arc<Store>,
    pub(crate) snapshots: Arc<crate::browse::snapshots::BrowseSnapshots>,
    pub(crate) files: Arc<FsCatalog>,
    pub(crate) metadata: MetadataQueue,
    pub(crate) preview: crate::resources::preview::PreviewQueue,
    pub(crate) tag_sync: Arc<TagXmpSync>,
    pub(crate) runtime: CoreRuntime,
    pub(crate) gate: parking_lot::Mutex<()>,
}

#[derive(Serialize, Deserialize)]
struct PathProof {
    path: PathBuf,
    revision: Option<String>,
}

impl FileService {
    /// Each item is an independent action. A later failure keeps earlier
    /// commits, and any uncertain item retains its recovery record.
    pub fn execute(
        self: &Arc<Self>,
        operation: FileOperation,
    ) -> Result<Receiver<Result<FileOperationResult, String>>, String> {
        let service = self.clone();
        self.runtime
            .submit("file-action", move || service.run(operation))
            .map_err(|error| error.to_string())
    }

    fn run(&self, operation: FileOperation) -> Result<FileOperationResult, String> {
        let _operation = self.gate.lock();
        let mut affected = Vec::new();
        for unit in units(operation) {
            if self.runtime.is_stopping() {
                return Err("application is shutting down".into());
            }
            let unit = normalize(unit)
                .map_err(|error| format!("{error}; {} paths already completed", affected.len()))?;
            super::validate_file_operation(&unit)
                .map_err(|error| format!("{error}; {} paths already completed", affected.len()))?;
            let id = repo::prepare(
                &self.store.write(),
                &serde_json::to_string(&unit).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            self.invalidate(&unit);
            let result = self.execute_one(id, &unit);
            // A failed filesystem action may have changed its first member.
            // Invalidation therefore runs on both the success and failure path.
            self.invalidate(&unit);
            match result {
                Ok(paths) => affected.extend(paths),
                Err(error) => {
                    let _ = repo::failure(&self.store.write(), id, &error);
                    return Err(format!(
                        "{error}; recovery record {id}; {} paths already completed",
                        affected.len()
                    ));
                }
            }
        }
        Ok(FileOperationResult {
            affected_paths: affected,
        })
    }

    fn execute_one(&self, id: i64, operation: &FileOperation) -> Result<Vec<PathBuf>, String> {
        let result = super::execute_file_operation(operation).map_err(|error| error.to_string())?;
        let mut paths = paths(operation);
        paths.extend(result.affected_paths.iter().cloned());
        paths.sort();
        paths.dedup();
        let proofs = paths
            .into_iter()
            .map(|path| {
                Ok(PathProof {
                    revision: revision(&path)?,
                    path,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let proofs = serde_json::to_string(&proofs).map_err(|error| error.to_string())?;
        repo::applied(&self.store.write(), id, &proofs).map_err(|error| error.to_string())?;
        file_action::commit(&self.store, id, operation).map_err(|error| error.to_string())?;
        self.tag_sync.schedule();
        Ok(result.affected_paths)
    }

    pub fn pending_recovery(&self) -> Result<Vec<FileOperationRecovery>, String> {
        repo::pending(&self.store.read())
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|row| {
                Ok(FileOperationRecovery {
                    id: row.id,
                    operation: serde_json::from_str(&row.operation)
                        .map_err(|error| error.to_string())?,
                    phase: row.phase,
                    detail: row.detail,
                })
            })
            .collect()
    }

    /// Recovery never repeats filesystem actions. A prepared or changed item
    /// requires manual inspection; an applied item can retry only its DB commit.
    pub fn recover(self: &Arc<Self>, id: i64) -> Result<Receiver<Result<(), String>>, String> {
        let service = self.clone();
        self.runtime
            .submit("file-action-recovery", move || service.recover_applied(id))
            .map_err(|error| error.to_string())
    }

    fn recover_applied(&self, id: i64) -> Result<(), String> {
        let _operation = self.gate.lock();
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let row = repo::pending(&self.store.read())
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|row| row.id == id)
            .ok_or_else(|| "recovery record no longer exists".to_owned())?;
        if row.phase != "filesApplied" {
            return Err(
                "filesystem action was interrupted; inspect files and sidecars before reconciling"
                    .into(),
            );
        }
        let proofs: Vec<PathProof> =
            serde_json::from_str(row.proofs.as_deref().ok_or("recovery proofs missing")?)
                .map_err(|error| error.to_string())?;
        for proof in proofs {
            if revision(&proof.path)? != proof.revision {
                return Err(
                    "filesystem changed since the action; recovery requires inspection".into(),
                );
            }
        }
        let operation = serde_json::from_str(&row.operation).map_err(|error| error.to_string())?;
        file_action::commit(&self.store, id, &operation).map_err(|error| error.to_string())?;
        self.invalidate(&operation);
        self.tag_sync.schedule();
        Ok(())
    }

    fn invalidate(&self, operation: &FileOperation) {
        for path in paths(operation) {
            let parent = path.parent().unwrap_or(&path);
            self.files.invalidate_directory(parent);
            self.metadata.invalidate_directory(parent);
            self.preview.invalidate_directory(parent);
            // Rebuildable state must not make a completed user action look
            // failed; it can be reconstructed by the next browse request.
            let _ = self.snapshots.invalidate_directory_snapshots(parent);
        }
    }
}

fn revision(path: &Path) -> Result<Option<String>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.is_dir() || metadata.file_type().is_symlink() {
        // No recursive discovery. This proof only guards the named object;
        // directory transfers are not accepted by this asset workflow.
        return Ok(Some(format!(
            "{:?}:{}:{:?}",
            metadata.file_type(),
            metadata.len(),
            metadata.modified().map_err(|error| error.to_string())?
        )));
    }
    super::observe_file(path)
        .map(|observed| Some(observed.revision_id()))
        .map_err(|error| error.to_string())
}

fn normalize(operation: FileOperation) -> Result<FileOperation, String> {
    fn source(path: PathBuf) -> Result<PathBuf, String> {
        let name = path.file_name().ok_or("invalid file name")?;
        // Resolve parents but keep the final link itself for safe deletion.
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Ok(parent
            .canonicalize()
            .map_err(|error| error.to_string())?
            .join(name))
    }
    fn assets(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
        paths
            .into_iter()
            .map(|path| {
                let path = source(path)?;
                if std::fs::symlink_metadata(&path)
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    return Err(
                        "asset transfer requires files; use root recovery for directories".into(),
                    );
                }
                Ok(path)
            })
            .collect()
    }
    Ok(match operation {
        FileOperation::Rename {
            source: path,
            new_name,
        } => FileOperation::Rename {
            source: assets(vec![path])?.remove(0),
            new_name,
        },
        FileOperation::Copy {
            sources,
            destination_dir,
        } => FileOperation::Copy {
            sources: assets(sources)?,
            destination_dir: super::relocation_directory(&destination_dir)
                .map_err(|error| error.to_string())?,
        },
        FileOperation::Move {
            sources,
            destination_dir,
        } => FileOperation::Move {
            sources: assets(sources)?,
            destination_dir: super::relocation_directory(&destination_dir)
                .map_err(|error| error.to_string())?,
        },
        FileOperation::Trash { paths } => FileOperation::Trash {
            paths: paths.into_iter().map(source).collect::<Result<_, _>>()?,
        },
        FileOperation::DeletePermanently { paths } => FileOperation::DeletePermanently {
            paths: paths.into_iter().map(source).collect::<Result<_, _>>()?,
        },
    })
}

fn units(operation: FileOperation) -> Vec<FileOperation> {
    match operation {
        FileOperation::Rename { .. } => vec![operation],
        FileOperation::Copy {
            sources,
            destination_dir,
        } => sources
            .into_iter()
            .map(|source| FileOperation::Copy {
                sources: vec![source],
                destination_dir: destination_dir.clone(),
            })
            .collect(),
        FileOperation::Move {
            sources,
            destination_dir,
        } => sources
            .into_iter()
            .map(|source| FileOperation::Move {
                sources: vec![source],
                destination_dir: destination_dir.clone(),
            })
            .collect(),
        FileOperation::Trash { paths } => paths
            .into_iter()
            .map(|path| FileOperation::Trash { paths: vec![path] })
            .collect(),
        FileOperation::DeletePermanently { paths } => paths
            .into_iter()
            .map(|path| FileOperation::DeletePermanently { paths: vec![path] })
            .collect(),
    }
}

fn paths(operation: &FileOperation) -> Vec<PathBuf> {
    match operation {
        FileOperation::Rename { source, new_name } => vec![
            source.clone(),
            source.parent().unwrap_or(Path::new("")).join(new_name),
        ],
        FileOperation::Copy {
            sources,
            destination_dir,
        }
        | FileOperation::Move {
            sources,
            destination_dir,
        } => sources
            .iter()
            .flat_map(|source| {
                [
                    source.clone(),
                    destination_dir.join(source.file_name().unwrap_or_default()),
                ]
            })
            .collect(),
        FileOperation::Trash { paths } | FileOperation::DeletePermanently { paths } => {
            paths.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppPaths, OxyApp};

    fn host(directory: &Path) -> OxyApp {
        OxyApp::open(AppPaths {
            data_dir: directory.join("data"),
            preview_dir: directory.join("cache"),
        })
        .unwrap()
        .0
    }

    #[test]
    fn same_folder_rename_preserves_current_global_identity_and_reference() {
        use crate::people::types::*;
        use std::time::Duration;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let source = root.join("photo.jpg");
        let destination = root.join("renamed.jpg");
        std::fs::write(&source, b"source photo").unwrap();
        let app = host(&root);
        let asset = app.person_service.tuple_asset(source.clone()).unwrap();
        let tuple = app
            .person_service
            .save_tuple_geometry(SavePersonTupleGeometry {
                folder_path: root.clone(),
                asset_path: source.clone(),
                instance_id: None,
                expected_revision: 0,
                source_revision: format!("{}:{}", asset.size_bytes, asset.modified_at_ms),
                face_box: Some([0.1, 0.1, 0.2, 0.2]),
                body_box: None,
                request_id: "geometry".into(),
            })
            .unwrap();
        let person = app
            .people
            .save_global_person(&SaveGlobalPerson {
                id: None,
                display_name: "Ada".into(),
                expected_revision: 0,
                request_id: "person".into(),
            })
            .unwrap();
        let tag = app.tags.create_custom_tag(None, "Ada").unwrap();
        app.person_service
            .set_global_person_tag(SetGlobalPersonTag {
                person_id: person.id.clone(),
                tag_id: Some(tag.id),
                expected_revision: person.revision,
                request_id: "tag".into(),
            })
            .unwrap();
        let workspace = app.person_service.workspace(root.clone()).unwrap();
        app.person_service
            .review_tuples(ReviewPersonTuples {
                folder_path: root.clone(),
                workspace_revision: workspace.revision,
                tuples: vec![tuple.clone()],
                person_id: person.id,
                decision: PersonReviewDecision::Belongs,
                replace_confirmed: false,
                request_id: "review".into(),
            })
            .unwrap();
        let person = app
            .people
            .global_people()
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        app.people
            .set_global_person_reference(&SetGlobalPersonReference {
                person_id: person.id.clone(),
                instance_id: tuple.id.clone(),
                enabled: true,
                expected_revision: person.revision,
                request_id: "reference".into(),
            })
            .unwrap();
        app.file_service
            .execute(FileOperation::Rename {
                source: source.clone(),
                new_name: "renamed.jpg".into(),
            })
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(!source.exists() && destination.exists());
        let workspace = app.person_service.workspace(root).unwrap();
        let moved = workspace
            .groups
            .iter()
            .flat_map(|group| &group.members)
            .find(|item| item.id == tuple.id)
            .unwrap();
        assert_eq!(moved.asset_path, destination);
        assert!(
            !moved.needs_review,
            "a verified rename must retain the current identity"
        );
        assert_eq!(
            moved.source_identity_revision,
            super::super::observe_file(&destination)
                .unwrap()
                .revision_id()
        );
        assert_eq!(moved.decision, Some(PersonReviewDecision::Belongs));
        assert_eq!(
            app.people
                .global_person_reference_tuples(&person.id)
                .unwrap()[0]
                .id,
            tuple.id
        );
        app.people
            .set_global_person_reference(&SetGlobalPersonReference {
                person_id: person.id,
                instance_id: tuple.id.clone(),
                enabled: true,
                expected_revision: person.revision + 1,
                request_id: "reference-after-rename".into(),
            })
            .unwrap();
        assert_eq!(
            app.tags
                .asset_tag_assignments(std::slice::from_ref(&destination))
                .unwrap()[0]
                .assigned_count,
            1
        );
        // A changed file has no continuity proof. Keep its user facts and
        // manual tag, but withdraw the derived person contribution.
        app.tags
            .set_asset_tag(std::slice::from_ref(&destination), tag.id, true)
            .unwrap();
        std::fs::write(&destination, b"modified source photo").unwrap();
        let changed_destination = directory.path().canonicalize().unwrap().join("changed.jpg");
        app.file_service
            .execute(FileOperation::Rename {
                source: destination,
                new_name: "changed.jpg".into(),
            })
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let changed = app
            .people
            .folder_people_workspace(changed_destination.parent().unwrap())
            .unwrap();
        let instance = changed
            .groups
            .iter()
            .flat_map(|group| &group.members)
            .find(|instance| instance.id == tuple.id)
            .unwrap();
        assert!(instance.needs_review);
        assert_eq!(instance.decision, Some(PersonReviewDecision::Belongs));
        assert_eq!(
            app.tags
                .asset_tag_source_kinds(&changed_destination, tag.id)
                .unwrap(),
            vec!["manual"]
        );
        assert!(app.shutdown(Duration::from_secs(5)));
    }

    #[test]
    fn a_failed_db_commit_survives_restart_and_recovers_without_moving_again() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let source = root.join("a.jpg");
        std::fs::write(&source, b"photo").unwrap();
        std::fs::write(super::super::sidecar_path(&source), b"sidecar").unwrap();
        let app = host(&root);
        let tag = app.tags.create_custom_tag(None, "kept").unwrap();
        app.tags
            .set_asset_tag(std::slice::from_ref(&source), tag.id, true)
            .unwrap();
        app.file_service.store.write().execute_batch("CREATE TRIGGER fail_file_rows BEFORE INSERT ON asset_tag_sources BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;").unwrap();
        let result = app
            .file_service
            .execute(FileOperation::Rename {
                source: source.clone(),
                new_name: "b.jpg".into(),
            })
            .unwrap()
            .recv()
            .unwrap();
        assert!(result.is_err());
        let record = app.file_service.pending_recovery().unwrap().remove(0);
        assert_eq!(record.phase, "filesApplied");
        assert!(!source.exists());
        let destination = root.join("b.jpg");
        assert_eq!(std::fs::read(&destination).unwrap(), b"photo");
        assert_eq!(
            std::fs::read(super::super::sidecar_path(&destination)).unwrap(),
            b"sidecar"
        );
        assert_eq!(
            app.tags
                .asset_tag_assignments(std::slice::from_ref(&source))
                .unwrap()[0]
                .assigned_count,
            1
        );
        assert!(app.shutdown(std::time::Duration::from_secs(3)));
        drop(app);
        let app = host(&root);
        app.file_service.store.clear_rebuildable_cache().unwrap();
        assert_eq!(app.file_service.pending_recovery().unwrap().len(), 1);
        app.file_service
            .store
            .write()
            .execute_batch("DROP TRIGGER fail_file_rows")
            .unwrap();
        app.file_service
            .recover(record.id)
            .unwrap()
            .recv()
            .unwrap()
            .unwrap();
        assert!(app.file_service.pending_recovery().unwrap().is_empty());
        assert_eq!(
            app.tags
                .asset_tag_assignments(std::slice::from_ref(&destination))
                .unwrap()[0]
                .assigned_count,
            1
        );
        assert!(app.shutdown(std::time::Duration::from_secs(3)));
    }

    #[test]
    fn changed_or_interrupted_recovery_never_repeats_a_filesystem_action() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let source = root.join("a.jpg");
        std::fs::write(&source, b"photo").unwrap();
        let app = host(&root);
        let operation = FileOperation::Rename {
            source: source.clone(),
            new_name: "b.jpg".into(),
        };
        let id = repo::prepare(
            &app.file_service.store.write(),
            &serde_json::to_string(&operation).unwrap(),
        )
        .unwrap();
        assert!(
            app.file_service
                .recover(id)
                .unwrap()
                .recv()
                .unwrap()
                .is_err()
        );
        assert!(source.exists());
        assert!(!root.join("b.jpg").exists());
        let proofs = vec![PathProof {
            path: source.clone(),
            revision: revision(&source).unwrap(),
        }];
        repo::applied(
            &app.file_service.store.write(),
            id,
            &serde_json::to_string(&proofs).unwrap(),
        )
        .unwrap();
        std::fs::write(&source, b"changed").unwrap();
        assert!(
            app.file_service
                .recover(id)
                .unwrap()
                .recv()
                .unwrap()
                .is_err()
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"changed");
        assert_eq!(app.file_service.pending_recovery().unwrap().len(), 1);
        assert!(app.shutdown(std::time::Duration::from_secs(3)));
    }

    #[test]
    fn sidecar_conflicts_are_checked_before_moving_and_partial_batches_keep_commits() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let source = root.join("a.jpg");
        std::fs::write(&source, b"photo").unwrap();
        std::fs::write(super::super::sidecar_path(&source), b"source sidecar").unwrap();
        std::fs::write(root.join("b.xmp"), b"existing destination").unwrap();
        let destination_dir = root.join("copies");
        std::fs::create_dir(&destination_dir).unwrap();
        let app = host(&root);
        let result = app
            .file_service
            .execute(FileOperation::Rename {
                source: source.clone(),
                new_name: "b.jpg".into(),
            })
            .unwrap()
            .recv()
            .unwrap();
        assert!(result.is_err());
        assert!(source.exists());
        assert!(!root.join("b.jpg").exists());
        assert!(app.file_service.pending_recovery().unwrap().is_empty());
        let result = app
            .file_service
            .execute(FileOperation::Copy {
                sources: vec![source, root.join("missing.jpg")],
                destination_dir: destination_dir.clone(),
            })
            .unwrap()
            .recv()
            .unwrap();
        assert!(result.unwrap_err().contains("paths already completed"));
        assert_eq!(
            std::fs::read(destination_dir.join("a.jpg")).unwrap(),
            b"photo"
        );
        assert!(app.file_service.pending_recovery().unwrap().is_empty());
        assert!(app.shutdown(std::time::Duration::from_secs(3)));
        assert!(
            app.file_service
                .execute(FileOperation::Trash { paths: vec![] })
                .is_err()
        );
    }
}
