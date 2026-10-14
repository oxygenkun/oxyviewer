//! Library use cases: root configuration, recovery and deferred indexing.
use super::{
    Library,
    scheduling::LibraryIndexQueue,
    types::{RootRelocationPlan, RootRelocationResult},
};
use crate::{
    app::workflows::root_relocation, resources::cache::CacheManager, runtime::CoreRuntime,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct LibraryService {
    pub(crate) library: Arc<Library>,
    pub(crate) snapshots: Arc<crate::browse::snapshots::BrowseSnapshots>,
    pub(crate) cache: Arc<CacheManager>,
    pub(crate) index: LibraryIndexQueue,
    pub(crate) runtime: CoreRuntime,
}

impl LibraryService {
    pub fn add_root(&self, path: &Path) -> Result<Vec<PathBuf>, String> {
        let root = path.canonicalize().map_err(|error| error.to_string())?;
        self.library
            .add_root(&root)
            .map_err(|error| error.to_string())?;
        self.index.schedule(self.runtime.clone(), root);
        self.roots()
    }

    pub fn remove_root(&self, path: &Path) -> Result<Vec<PathBuf>, String> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        self.snapshots
            .invalidate_snapshot_root(&path)
            .map_err(|error| error.to_string())?;
        self.library
            .remove_root(&path)
            .map_err(|error| error.to_string())?;
        self.roots()
    }

    pub fn roots(&self) -> Result<Vec<PathBuf>, String> {
        self.library.roots().map_err(|error| error.to_string())
    }

    pub fn reorder_roots(&self, paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
        self.library
            .reorder_roots(paths)
            .map_err(|error| error.to_string())?;
        self.roots()
    }

    pub fn check_root(path: &Path) -> Result<(), String> {
        crate::files::relocation_directory(path)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn plan_relocation(&self, old: &Path, new: &Path) -> Result<RootRelocationPlan, String> {
        let cache = oxy_media::DiskMediaCache::new(self.cache.preview_dir(), 256)
            .map_err(|error| error.to_string())?;
        let sources = cache
            .relocation_sources(old)
            .map_err(|error| error.to_string())?;
        let proofs = sources
            .into_iter()
            .map(|source| (source.canonical_path, source.revision_id))
            .collect::<Vec<_>>();
        root_relocation::preview(&self.library, old, new, &proofs)
            .map_err(|error| error.to_string())
    }

    /// The workflow revalidates the preview before atomically updating user
    /// records. Artifact reuse happens afterwards and reports partial failure.
    pub fn relocate(&self, plan: &RootRelocationPlan) -> Result<RootRelocationResult, String> {
        let cache = oxy_media::DiskMediaCache::new(self.cache.preview_dir(), 256)
            .map_err(|error| error.to_string())?;
        let sources = cache
            .relocation_sources(&plan.old_root)
            .map_err(|error| error.to_string())?;
        let proofs = sources
            .iter()
            .map(|source| (source.canonical_path.clone(), source.revision_id.clone()))
            .collect::<Vec<_>>();
        root_relocation::commit(&self.library, &self.snapshots, plan, &proofs)
            .map_err(|error| error.to_string())?;
        let report = root_relocation::reuse_root_artifacts(&cache, plan, &sources);
        self.index
            .schedule(self.runtime.clone(), report.root_path.clone());
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppPaths, OxyApp};

    #[test]
    fn headless_recovery_moves_user_tags_and_rejects_a_changed_preview() {
        let directory = tempfile::tempdir().unwrap();
        let old = directory.path().join("old");
        std::fs::create_dir(&old).unwrap();
        let old = old.canonicalize().unwrap();
        let photo = old.join("a.jpg");
        std::fs::write(&photo, b"original").unwrap();
        let (app, _) = OxyApp::open(AppPaths {
            data_dir: directory.path().join("data"),
            preview_dir: directory.path().join("cache"),
        })
        .unwrap();
        app.library_service.add_root(&old).unwrap();
        let tag = app.tags.create_custom_tag(None, "reviewed").unwrap();
        app.tags
            .set_asset_tag(std::slice::from_ref(&photo), tag.id, true)
            .unwrap();
        let new = directory.path().join("new");
        std::fs::rename(&old, &new).unwrap();
        let new = new.canonicalize().unwrap();
        let plan = app.library_service.plan_relocation(&old, &new).unwrap();
        std::fs::write(new.join("a.jpg"), b"changed after preview").unwrap();
        assert!(app.library_service.relocate(&plan).is_err());
        assert_eq!(
            app.library_service.roots().unwrap(),
            std::slice::from_ref(&old)
        );
        let plan = app.library_service.plan_relocation(&old, &new).unwrap();
        let report = app.library_service.relocate(&plan).unwrap();
        assert_eq!(report.root_path, new);
        assert_eq!(report.needs_review, 1);
        assert_eq!(
            app.tags
                .asset_tag_assignments(&[new.join("a.jpg")])
                .unwrap()[0]
                .assigned_count,
            1
        );
        assert!(app.shutdown(std::time::Duration::from_secs(3)));
    }
}
