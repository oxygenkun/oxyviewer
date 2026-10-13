//! Application commit after a recorded, explicit filesystem action.
use crate::{
    files::{repo, types::FileOperation},
    storage::{Store, StoreError},
};
use std::path::Path;

pub(crate) fn commit(
    store: &Store,
    recovery_id: i64,
    operation: &FileOperation,
) -> Result<(), StoreError> {
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
