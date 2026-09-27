//! Explicit, confirmed root recovery. User records move atomically; derived
//! browsing state is invalidated, never used as the source of user facts.
use crate::{Library, LibraryError, cache::index::forget_root};
use oxy_domain::{RelocationEntry, RelocationMatch, RootRelocationPlan};
use oxy_store::{
    Connection,
    repo::{self, cross::relocation as rows},
};
use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
};

fn invalid(message: &str) -> LibraryError {
    LibraryError::InvalidRelocation(message.into())
}

impl Library {
    pub fn plan_root_relocation(
        &self,
        old: &Path,
        new: &Path,
        cached: &[(PathBuf, String)],
    ) -> Result<RootRelocationPlan, LibraryError> {
        plan(&self.read_connection(), old, new, cached)
    }

    pub fn relocate_root(
        &self,
        expected: &RootRelocationPlan,
        cached: &[(PathBuf, String)],
    ) -> Result<(), LibraryError> {
        let _index = self.index_gate.lock();
        self.invalidate_snapshot_root(&expected.old_root)?;
        self.invalidate_snapshot_root(&expected.new_root)?;
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let current = plan(&transaction, &expected.old_root, &expected.new_root, cached)?;
        if current != *expected {
            return Err(invalid("files changed since preview; inspect the new plan"));
        }
        let folders = rows::folder_paths(&transaction)?;
        for entry in &current.entries {
            let source = entry.old_path.to_string_lossy();
            let destination = entry.new_path.to_string_lossy();
            rows::move_asset(&transaction, &source, &destination)?;
            let verified = entry.status == RelocationMatch::Verified;
            rows::rebind_instances(
                &transaction,
                &destination,
                entry.old_identity_revision.as_deref(),
                entry.new_identity_revision.as_deref(),
                verified,
            )?;
            if let (true, Some(old), Some(new)) = (
                verified,
                &entry.old_identity_revision,
                &entry.new_identity_revision,
            ) {
                let folder = entry
                    .new_path
                    .parent()
                    .ok_or_else(|| invalid("invalid asset path"))?
                    .to_string_lossy();
                rows::rebind_features(&transaction, &source, &destination, &folder, old, new)?;
                rows::rebind_detections(&transaction, &source, &destination, &folder, old, new)?;
            }
        }
        for folder in folders {
            if let Ok(relative) = Path::new(&folder).strip_prefix(&current.old_root) {
                let destination = current.new_root.join(relative);
                rows::move_folder(&transaction, &folder, &destination.to_string_lossy())?;
                for subject in rows::subjects(&transaction, &destination.to_string_lossy())? {
                    repo::cross::reconcile_person_sources_for_subject(&transaction, &subject)?;
                }
            }
        }
        rows::replace_root(
            &transaction,
            &current.old_root.to_string_lossy(),
            &current.new_root.to_string_lossy(),
        )?;
        forget_root(&transaction, &current.old_root.to_string_lossy())?;
        // Filesystem state is not covered by SQLite's transaction. Recheck at
        // publication so edits during the mapping loop abort every user write.
        for entry in &current.entries {
            let observed = match oxy_fs::observe_file(&entry.new_path) {
                Ok(source) => Some(source.revision_id()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if observed != entry.new_identity_revision {
                return Err(invalid("files changed while relocating; retry the preview"));
            }
        }
        transaction.commit()?;
        Ok(())
    }
}

fn plan(
    connection: &Connection,
    old: &Path,
    new: &Path,
    cached: &[(PathBuf, String)],
) -> Result<RootRelocationPlan, LibraryError> {
    let new = oxy_fs::relocation_directory(new)?;
    if old == new || new.starts_with(old) || old.starts_with(&new) {
        return Err(invalid(
            "source and destination must be separate directories",
        ));
    }
    let roots = repo::library::list_roots(connection)?;
    if !roots.iter().any(|root| Path::new(root) == old) {
        return Err(invalid("source is no longer registered"));
    }
    if roots.iter().any(|root| {
        Path::new(root) != old && (new.starts_with(root) || Path::new(root).starts_with(&new))
    }) {
        return Err(invalid("destination overlaps an existing root"));
    }
    if roots.iter().any(|root| {
        Path::new(root) != old && (old.starts_with(root) || Path::new(root).starts_with(old))
    }) {
        return Err(invalid(
            "source overlaps another registered root; remove the duplicate entry first",
        ));
    }
    let cached: HashSet<_> = cached.iter().cloned().collect();
    let paths = rows::asset_paths(connection)?;
    if paths
        .iter()
        .chain(rows::folder_paths(connection)?.iter())
        .any(|path| Path::new(path).starts_with(&new))
    {
        return Err(invalid(
            "destination already has library records; merging requires separate review",
        ));
    }
    let paths: BTreeSet<PathBuf> = paths
        .into_iter()
        .map(PathBuf::from)
        .chain(cached.iter().map(|(path, _)| path.clone()))
        .filter(|path| path.starts_with(old))
        .collect();
    let mut entries = Vec::new();
    for path in paths {
        let relative = path
            .strip_prefix(old)
            .map_err(|_| invalid("invalid source path"))?;
        let destination = new.join(relative);
        let mut entry = RelocationEntry {
            old_path: path.clone(),
            new_path: destination.clone(),
            status: RelocationMatch::Missing,
            old_identity_revision: None,
            new_identity_revision: None,
        };
        match oxy_fs::observe_file(&destination) {
            Ok(mut observed) => {
                if observed.canonical_path != destination {
                    return Err(invalid(
                        "destination contains a redirected asset; resolve it before relocating",
                    ));
                }
                entry.new_identity_revision = Some(observed.revision_id());
                observed.canonical_path = path.clone();
                let previous = observed.revision_id();
                let evidence = rows::identity_revisions(connection, &path.to_string_lossy())?;
                let verified = evidence.contains(&previous)
                    || cached.contains(&(path.clone(), previous.clone()));
                entry.status = if verified {
                    RelocationMatch::Verified
                } else {
                    RelocationMatch::Unverified
                };
                entry.old_identity_revision = Some(previous);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        entries.push(entry);
    }
    Ok(RootRelocationPlan {
        old_root: old.to_owned(),
        new_root: new,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    type Fixture = (
        tempfile::TempDir,
        Library,
        PathBuf,
        PathBuf,
        Vec<(PathBuf, String)>,
    );

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        fs::create_dir(&old).unwrap();
        let old = old.canonicalize().unwrap();
        let library = Library::open(&dir.path().join("library.db")).unwrap();
        library.add_root(&old).unwrap();
        let mut proofs = Vec::new();
        {
            let db = library.write();
            db.execute("INSERT INTO folder_people(id,folder_path,display_name,identity_confirmed) VALUES('person',?1,'Alice',1)", [old.to_string_lossy().as_ref()]).unwrap();
            db.execute(
                "INSERT INTO custom_tags(id,name,name_key,sort_order) VALUES(1,'tag','tag',0)",
                [],
            )
            .unwrap();
            for name in ["same", "changed", "missing"] {
                let file = old.join(format!("{name}.jpg"));
                fs::write(&file, name).unwrap();
                let revision = oxy_fs::observe_file(&file).unwrap().revision_id();
                proofs.push((file.clone(), revision.clone()));
                db.execute("INSERT INTO person_manual_instances(id,folder_path,asset_path,source_revision,source_identity_revision) VALUES(?1,?2,?3,'10:20',?4)", rusqlite::params![name, old.to_string_lossy(), file.to_string_lossy(),revision]).unwrap();
                db.execute("INSERT INTO person_review_decisions(instance_id,subject_id,decision) VALUES(?1,'person','belongs')", [name]).unwrap();
                db.execute("INSERT INTO person_references(subject_id,instance_id,source_revision) VALUES('person',?1,'10:20')", [name]).unwrap();
                db.execute(
                    "INSERT INTO asset_tags(asset_path,tag_id) VALUES(?1,1)",
                    [file.to_string_lossy().as_ref()],
                )
                .unwrap();
                db.execute("INSERT INTO asset_tag_sources(asset_path,tag_id,source_kind,source_id) VALUES(?1,1,'manual','')", [file.to_string_lossy().as_ref()]).unwrap();
            }
        }
        let new = dir.path().join("new");
        fs::rename(&old, &new).unwrap();
        let new = new.canonicalize().unwrap();
        fs::write(new.join("changed.jpg"), "different file contents").unwrap();
        fs::remove_file(new.join("missing.jpg")).unwrap();
        (dir, library, old, new, proofs)
    }

    #[test]
    fn relocation_preserves_annotations_marks_uncertainty_and_survives_restart() {
        let (dir, library, old, new, proofs) = fixture();
        let plan = library.plan_root_relocation(&old, &new, &proofs).unwrap();
        for (name, status) in [
            ("same", RelocationMatch::Verified),
            ("changed", RelocationMatch::Unverified),
            ("missing", RelocationMatch::Missing),
        ] {
            assert_eq!(
                plan.entries
                    .iter()
                    .find(|e| e.old_path == old.join(format!("{name}.jpg")))
                    .unwrap()
                    .status,
                status
            );
        }
        library.relocate_root(&plan, &proofs).unwrap();
        drop(library);
        let library = Library::open(&dir.path().join("library.db")).unwrap();
        assert_eq!(library.roots().unwrap(), [new.clone()]);
        {
            let db = library.read_connection();
            for (name, review) in [("same", false), ("changed", true), ("missing", true)] {
                let (path,folder,needs_review): (String,String,bool) = db.query_row("SELECT asset_path,folder_path,needs_review FROM person_manual_instances WHERE id=?1", [name], |row| Ok((row.get("asset_path")?,row.get("folder_path")?,row.get("needs_review")?))).unwrap();
                assert_eq!(Path::new(&path), new.join(format!("{name}.jpg")));
                assert_eq!(Path::new(&folder), new);
                assert_eq!(needs_review, review);
            }
            for table in [
                "person_review_decisions",
                "person_references",
                "asset_tags",
                "asset_tag_sources",
            ] {
                let count: i64 = db
                    .query_row(
                        &format!("SELECT COUNT(*) AS count FROM {table}"),
                        [],
                        |row| row.get("count"),
                    )
                    .unwrap();
                assert_eq!(count, 3, "{table}");
            }
        }
        // Removing an unavailable list entry must not delete annotations.
        fs::rename(&new, dir.path().join("offline")).unwrap();
        library.remove_root(&new).unwrap();
        assert!(library.roots().unwrap().is_empty());
        let count: i64 = library
            .read_connection()
            .query_row(
                "SELECT COUNT(*) AS count FROM person_manual_instances",
                [],
                |row| row.get("count"),
            )
            .unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn relocation_rolls_back_all_records_on_a_mid_transaction_error() {
        let (_dir, library, old, new, proofs) = fixture();
        let plan = library.plan_root_relocation(&old, &new, &proofs).unwrap();
        library.write().execute_batch("CREATE TRIGGER test_abort_relocation BEFORE UPDATE OF asset_path ON person_manual_instances WHEN OLD.id='same' BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
        assert!(library.relocate_root(&plan, &proofs).is_err());
        assert_eq!(library.roots().unwrap(), [old.clone()]);
        let paths = rows::asset_paths(&library.read_connection()).unwrap();
        assert!(paths.iter().all(|path| Path::new(path).starts_with(&old)));
    }

    #[test]
    fn relocation_only_rebinds_verified_detection_and_feature_rows_and_fences_workers() {
        let (_dir, library, old, new, proofs) = fixture();
        {
            let db = library.write();
            db.execute("INSERT INTO person_feature_spaces(id,modality,dimension,producer_fingerprint) VALUES('space','face',1,'producer')",[]).unwrap();
            db.execute("INSERT INTO person_analysis_heads(folder_path,generation,run_id) VALUES(?1,1,'run')",[old.to_string_lossy().as_ref()]).unwrap();
            db.execute("INSERT INTO person_analysis_runs(run_id,folder_path,pipeline_id,pipeline_fingerprint,generation,state) VALUES('run',?1,'pipeline','fingerprint',1,'running')",[old.to_string_lossy().as_ref()]).unwrap();
            for (path, revision) in &proofs {
                db.execute("INSERT INTO person_instances_cache(folder_path,asset_path,instance_id,source_revision,producer_fingerprint,pipeline_fingerprint,run_id) VALUES(?1,?2,'instance',?3,'producer','fingerprint','run')",rusqlite::params![old.to_string_lossy(),path.to_string_lossy(),revision]).unwrap();
                db.execute("INSERT INTO person_features_cache(folder_path,asset_path,instance_id,source_revision,feature_space_id,pipeline_fingerprint,vector) VALUES(?1,?2,'instance',?3,'space','fingerprint',X'0000803F')",rusqlite::params![old.to_string_lossy(),path.to_string_lossy(),revision]).unwrap();
            }
        }
        let plan = library.plan_root_relocation(&old, &new, &proofs).unwrap();
        library.relocate_root(&plan, &proofs).unwrap();
        let db = library.read_connection();
        for table in ["person_instances_cache", "person_features_cache"] {
            let (path, revision): (String, String) = db
                .query_row(
                    &format!("SELECT asset_path,source_revision FROM {table} WHERE folder_path=?1"),
                    [new.to_string_lossy().as_ref()],
                    |row| Ok((row.get("asset_path")?, row.get("source_revision")?)),
                )
                .unwrap();
            assert_eq!(Path::new(&path), new.join("same.jpg"));
            assert_eq!(
                revision,
                oxy_fs::observe_file(&new.join("same.jpg"))
                    .unwrap()
                    .revision_id()
            );
            let count: i64 = db
                .query_row(
                    &format!("SELECT COUNT(*) AS count FROM {table} WHERE folder_path=?1"),
                    [old.to_string_lossy().as_ref()],
                    |row| row.get("count"),
                )
                .unwrap();
            assert_eq!(count, 2);
        }
        assert!(!repo::person_cache::analysis_is_current(&db, "run").unwrap());
    }

    #[test]
    fn relocation_rejects_stale_preview_and_conflicting_destination_without_writes() {
        let (_dir, library, old, new, proofs) = fixture();
        let plan = library.plan_root_relocation(&old, &new, &proofs).unwrap();
        fs::write(new.join("same.jpg"), "modified after preview").unwrap();
        assert!(library.relocate_root(&plan, &proofs).is_err());
        assert_eq!(library.roots().unwrap(), [old.clone()]);
        let current = library.plan_root_relocation(&old, &new, &proofs).unwrap();
        library.add_root(&new).unwrap();
        assert!(library.relocate_root(&current, &proofs).is_err());
        let path: String = library
            .read_connection()
            .query_row(
                "SELECT asset_path FROM person_manual_instances WHERE id='same'",
                [],
                |row| row.get("asset_path"),
            )
            .unwrap();
        assert_eq!(Path::new(&path), old.join("same.jpg"));
    }
}
