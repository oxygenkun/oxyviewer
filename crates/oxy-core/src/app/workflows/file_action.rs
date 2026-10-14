//! Application commit after a recorded, explicit filesystem action.
use crate::{
    files::{repo, types::FileOperation},
    storage::{Store, StoreError},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub(crate) fn commit(
    store: &Store,
    recovery_id: i64,
    operation: &FileOperation,
) -> Result<(), StoreError> {
    // Observe before acquiring the database writer. Rebase the target's physical
    // identity to the previous path, just as confirmed root recovery does.
    let mut identities = HashMap::new();
    match operation {
        FileOperation::Rename { source, new_name } => {
            let destination = source.parent().expect("validated parent").join(new_name);
            identities.insert(source.clone(), renamed_identity(source, &destination)?);
        }
        FileOperation::Move {
            sources,
            destination_dir,
        } => {
            for source in sources
                .iter()
                .filter(|source| source.parent() == Some(destination_dir.as_path()))
            {
                let destination = destination_dir.join(source.file_name().expect("validated name"));
                identities.insert(source.clone(), renamed_identity(source, &destination)?);
            }
        }
        _ => {}
    }
    let mut connection = store.write();
    let transaction = connection.transaction()?;
    match operation {
        FileOperation::Rename { source, new_name } => {
            let destination = source.parent().expect("validated parent").join(new_name);
            super::asset_move::relocate_asset(
                &transaction,
                &source.to_string_lossy(),
                &destination.to_string_lossy(),
                true,
            )?;
            rebind_renamed_identity(&transaction, source, &destination, &identities[source])?;
        }
        FileOperation::Copy {
            sources,
            destination_dir,
        }
        | FileOperation::Move {
            sources,
            destination_dir,
        } => {
            for source in sources {
                let destination = destination_dir.join(source.file_name().expect("validated name"));
                if matches!(operation, FileOperation::Move { .. }) {
                    super::asset_move::relocate_asset(
                        &transaction,
                        &source.to_string_lossy(),
                        &destination.to_string_lossy(),
                        source.parent() == destination.parent(),
                    )?;
                    if let Some(identity) = identities.get(source) {
                        rebind_renamed_identity(&transaction, source, &destination, identity)?;
                    }
                } else {
                    crate::tags::repo::copy_source_rows(
                        &transaction,
                        &source.to_string_lossy(),
                        &destination.to_string_lossy(),
                        false,
                    )?;
                    crate::tags::repo::copy_effective_rows(
                        &transaction,
                        &source.to_string_lossy(),
                        &destination.to_string_lossy(),
                        false,
                    )?;
                    crate::tags::repo::enqueue_sync(
                        &transaction,
                        &[destination.to_string_lossy().into_owned()],
                    )?;
                }
            }
        }
        FileOperation::Trash { paths } | FileOperation::DeletePermanently { paths } => {
            let known = super::root_relocation::queries::asset_paths(&transaction)?;
            for asset in known {
                if paths.iter().any(|path| Path::new(&asset).starts_with(path)) {
                    super::asset_delete::forget_asset(&transaction, &asset)?;
                }
            }
        }
    }
    // Application rows and removal of the recovery record commit together.
    repo::complete(&transaction, recovery_id)?;
    transaction.commit()?;
    Ok(())
}

struct RenamedIdentity {
    previous: String,
    current: String,
}

fn renamed_identity(source: &Path, destination: &Path) -> Result<RenamedIdentity, StoreError> {
    let mut observed = crate::files::observe_file(destination)?;
    if observed.canonical_path != destination {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "destination path was redirected after the file action",
        )
        .into());
    }
    let current = observed.revision_id();
    observed.canonical_path = PathBuf::from(source);
    Ok(RenamedIdentity {
        previous: observed.revision_id(),
        current,
    })
}

fn rebind_renamed_identity(
    connection: &crate::storage::Connection,
    source: &Path,
    destination: &Path,
    identity: &RenamedIdentity,
) -> Result<(), StoreError> {
    let source = source.to_string_lossy();
    let destination = destination.to_string_lossy();
    let folder = Path::new(destination.as_ref())
        .parent()
        .expect("validated parent")
        .to_string_lossy();
    crate::people::repo::relocation::rebind_instances(
        connection,
        &destination,
        Some(&identity.previous),
        Some(&identity.current),
        true,
    )?;
    crate::people::repo::relocation::rebind_features(
        connection,
        &destination,
        &destination,
        &folder,
        &identity.previous,
        &identity.current,
    )?;
    crate::people::repo::relocation::rebind_detections(
        connection,
        &source,
        &destination,
        &folder,
        &identity.previous,
        &identity.current,
    )?;
    crate::people::repo::relocation::forget_mismatched_features(
        connection,
        &destination,
        &identity.current,
    )?;
    crate::people::repo::clusters::forget_snapshot(connection, &folder)?;
    crate::people::repo::global::forget_suggestions(connection, &folder)?;
    for person in crate::people::repo::global::people_for_asset(connection, &destination)? {
        super::person_tags::reconcile_global_person_sources(connection, &person)?;
    }
    Ok(())
}
