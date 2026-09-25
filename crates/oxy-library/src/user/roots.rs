//! Explicit library roots: user configuration, not derived state.
//!
//! A root exists only because the user pointed OxyViewer at it. Nothing in
//! this module is rebuilt by a scan, and removing a root is a user decision
//! that never cascades into identifiers, reviews, or tags. The table declares
//! itself `user` in `oxy_store::schema::user`, which is what keeps a cache
//! clear away from it; what stays here is what a root *means*: an order the
//! user chose, a canonical path, and a removal that also drops the index rows
//! derived from it.

use crate::{Library, LibraryError, cache::index::forget_root};
use oxy_store::repo;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

impl Library {
    pub fn add_root(&self, path: &Path) -> Result<(), LibraryError> {
        let canonical = path.canonicalize()?;
        repo::library::insert_root(&self.write(), &canonical.to_string_lossy())?;
        Ok(())
    }

    pub fn remove_root(&self, path: &Path) -> Result<(), LibraryError> {
        // Stored roots are canonical, but removal must also work after a folder
        // has been moved or disconnected. In that case the exact persisted path
        // is still safe to remove.
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.invalidate_snapshot_root(&path)?;
        let root = path.to_string_lossy();
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        repo::library::delete_root(&transaction, root.as_ref())?;
        // The index owns its own rows. Asking it to forget the root keeps the
        // delete list next to the schema that defines those tables.
        forget_root(&transaction, root.as_ref())?;
        transaction.commit()?;
        Ok(())
    }

    pub fn roots(&self) -> Result<Vec<PathBuf>, LibraryError> {
        Ok(repo::library::list_roots(&self.read_connection())?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    pub fn reorder_roots(&self, paths: &[PathBuf]) -> Result<(), LibraryError> {
        let mut connection = self.write();
        let stored = repo::library::root_paths(&connection)?;
        let requested = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<HashSet<_>>();
        if stored.len() != paths.len() || requested != stored {
            return Err(LibraryError::InvalidRootOrder);
        }

        let transaction = connection.transaction()?;
        for (sort_order, path) in paths.iter().enumerate() {
            repo::library::set_sort_order(&transaction, &path.to_string_lossy(), sort_order as i64)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn contains_root(&self, path: &Path) -> Result<bool, LibraryError> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        Ok(repo::library::contains_root(
            &self.read_connection(),
            &path.to_string_lossy(),
        )?)
    }
}
