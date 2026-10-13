//! Folder browsing and application query/invalidation coordination.
use crate::browse::types::{
    AssetQuery, BrowsePage, DirectoryBrowseProgress, DirectorySearchMatch, DirectoryTreeSnapshot,
    FolderSession,
};
use crate::resources::{
    LibraryIndexQueue, directory_tree::DirectoryTreeQueue, metadata::MetadataQueue,
    preview::PreviewQueue,
};
use crate::{
    events::CoreEvent,
    files::FsCatalog,
    library::Library,
    metadata::MetadataFacade,
    people::People,
    runtime::CoreRuntime,
    tags::{Tags, xmp_sync::TagXmpSync},
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub struct BrowseService {
    pub(crate) files: Arc<FsCatalog>,
    pub(crate) library: Arc<Library>,
    pub(crate) projections: Arc<crate::resources::projections::ResourceProjections>,
    pub(crate) metadata: MetadataFacade,
    pub(crate) tags: Arc<Tags>,
    pub(crate) people: Arc<People>,
    pub(crate) metadata_queue: MetadataQueue,
    pub(crate) preview_queue: PreviewQueue,
    pub(crate) directory_tree_queue: DirectoryTreeQueue,
    pub(crate) library_index_queue: LibraryIndexQueue,
    pub(crate) runtime: CoreRuntime,
    pub(crate) tag_sync: Arc<TagXmpSync>,
}

impl BrowseService {
    pub fn open_folder(&self, path: PathBuf) -> Result<FolderSession, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let files = self.files.clone();
        let library = self.library.clone();
        let (session, should_index) = {
            let _foreground = library.foreground.enter();
            let registered = library
                .roots()
                .map_err(|error| error.to_string())?
                .contains(&path);
            let session = if registered {
                files.restore_registered_folder(path)
            } else {
                files.open_folder(path).map_err(|error| error.to_string())?
            };
            let should_index = library
                .root_needs_index(&session.root_path)
                .unwrap_or(false);
            Ok::<_, String>((session, should_index))
        }?;
        if should_index {
            self.library_index_queue
                .schedule(self.runtime.clone(), session.root_path.clone());
        }
        self.tag_sync.schedule();
        Ok(session)
    }

    pub fn list_assets(
        &self,
        session_id: String,
        directory: Option<PathBuf>,
        query: AssetQuery,
        cursor: Option<usize>,
        snapshot_revision: Option<u64>,
    ) -> Result<BrowsePage, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let app = self.runtime.clone();
        let files = self.files.clone();
        let metadata = self.metadata.clone();
        let library = self.library.clone();
        let tags = self.tags.clone();
        let people = self.people.clone();
        {
            let _foreground = library.foreground.enter();
            let started = Instant::now();
            let root = files
                .session_root(&session_id)
                .map_err(|error| error.to_string())?;
            let directory = directory.unwrap_or_else(|| root.clone());
            let mut progress = DirectoryBrowseProgress {
                session_id,
                directory: directory.clone(),
                stage: "cache".into(),
                ..Default::default()
            };
            let _ = app.publish(CoreEvent::DirectoryBrowseProgress(progress.clone()));
            // Preserve existing library-wide search semantics. Ordinary browsing
            // uses independent snapshots even during an incomplete root index.
            if query.person_filter.is_none()
                && query.person_cluster_filter.is_none()
                && query.person_tuple_filter.is_none()
                && query
                    .search
                    .as_ref()
                    .is_some_and(|search| !search.is_empty())
                && let Some(page) = library
                    .list_assets(&root, &directory, &query, cursor.unwrap_or(0))
                    .map_err(|error| error.to_string())?
            {
                progress.source = "index".into();
                progress.stage = "ready".into();
                progress.cache_ms = started.elapsed().as_millis() as u64;
                progress.elapsed_ms = progress.cache_ms;
                return Ok(BrowsePage {
                    page,
                    progress,
                    snapshot_revision: None,
                });
            }
            let mut last_report = Instant::now();
            let mut scanning = false;
            let read = library
                .browse_directory_revision(&root, &directory, snapshot_revision, |scan| {
                    if !scanning {
                        progress.cache_ms = started.elapsed().as_millis() as u64;
                        scanning = true;
                    }
                    progress.stage = if scan.resolving {
                        "resolving"
                    } else if scan.reading_attributes {
                        "attributes"
                    } else {
                        "enumerating"
                    }
                    .into();
                    progress.discovered_count = scan.discovered_count;
                    progress.enumeration_ms = scan.enumeration_ms;
                    progress.resolve_ms = scan.resolve_ms;
                    progress.attributes_ms = scan.attributes_ms;
                    progress.elapsed_ms = started.elapsed().as_millis() as u64;
                    if last_report.elapsed() >= Duration::from_millis(100) {
                        let _ = app.publish(CoreEvent::DirectoryBrowseProgress(progress.clone()));
                        last_report = Instant::now();
                    }
                })
                .map_err(|error| error.to_string())?;
            if !scanning {
                progress.cache_ms = started.elapsed().as_millis() as u64;
            }
            progress.snapshot_serialize_ms = read.snapshot_serialize_ms;
            progress.snapshot_persist_ms = read.snapshot_persist_ms;
            progress.source = read.source.into();
            progress.stage = "sorting".into();
            let _ = app.publish(CoreEvent::DirectoryBrowseProgress(progress.clone()));
            let sort_started = Instant::now();
            // A metadata filter cannot be answered from the cheap directory scan:
            // flags reach the summary only through the sidecar/XMP enrichment pass.
            let tagged_assets = if query.tag_ids.is_empty() {
                None
            } else {
                Some(
                    tags.filter_assets_by_tags(&read.assets, &query)
                        .map_err(|error| error.to_string())?,
                )
            };
            let candidates = tagged_assets.as_deref().unwrap_or(&read.assets);
            let person_assets = query
                .person_filter
                .as_ref()
                .map(|filter| people.filter_assets_by_person(&directory, candidates, filter))
                .transpose()
                .map_err(|error| error.to_string())?;
            let candidates = person_assets.as_deref().unwrap_or(candidates);
            let cluster_assets = query
                .person_cluster_filter
                .as_ref()
                .map(|filter| people.filter_assets_by_cluster(&directory, candidates, filter))
                .transpose()
                .map_err(|error| error.to_string())?;
            let candidates = cluster_assets.as_deref().unwrap_or(candidates);
            let tuple_assets = query
                .person_tuple_filter
                .as_ref()
                .map(|filter| people.filter_assets_by_tuple(&directory, candidates, filter))
                .transpose()
                .map_err(|error| error.to_string())?;
            let candidates = tuple_assets.as_deref().unwrap_or(candidates);
            let page = if query.needs_metadata_enrichment() {
                let mut assets = candidates.to_vec();
                metadata
                    .enrich_summaries(&mut assets)
                    .map_err(|error| error.to_string())?;
                crate::files::page_assets(&assets, &query, cursor.unwrap_or(0))
            } else {
                crate::files::page_assets(candidates, &query, cursor.unwrap_or(0))
            };
            progress.sort_ms = sort_started.elapsed().as_millis() as u64;
            progress.discovered_count = read.assets.len();
            progress.stage = if read.error.is_some() {
                "stale"
            } else {
                "ready"
            }
            .into();
            progress.error = read.error;
            progress.elapsed_ms = started.elapsed().as_millis() as u64;
            let _ = app.publish(CoreEvent::DirectoryBrowseProgress(progress.clone()));
            if read.needs_validation {
                let library = library.clone();
                let mut background = progress.clone();
                let _ = app.clone().spawn_blocking(move || {
                    let started = Instant::now();
                    let mut last_report = Instant::now();
                    let result =
                        library.revalidate_directory(&root, &directory, read.epoch, |scan| {
                            background.stage = "validating".into();
                            background.discovered_count = scan.discovered_count;
                            background.enumeration_ms = scan.enumeration_ms;
                            background.resolve_ms = scan.resolve_ms;
                            background.attributes_ms = scan.attributes_ms;
                            if last_report.elapsed() >= Duration::from_millis(100) {
                                let _ = app.publish(CoreEvent::DirectoryBrowseProgress(
                                    background.clone(),
                                ));
                                last_report = Instant::now();
                            }
                        });
                    background.elapsed_ms = started.elapsed().as_millis() as u64;
                    match result {
                        Ok(true) => background.stage = "updated".into(),
                        Ok(false) => return,
                        Err(error) => {
                            background.stage = "stale".into();
                            background.error = Some(error.to_string());
                        }
                    }
                    let _ = app.publish(CoreEvent::DirectoryBrowseProgress(background));
                });
            }
            Ok(BrowsePage {
                page,
                progress,
                snapshot_revision: Some(read.revision),
            })
        }
    }

    pub fn get_directory_tree(&self, session_id: String) -> Result<DirectoryTreeSnapshot, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let files = self.files.clone();
        {
            files
                .directory_tree(&session_id)
                .map_err(|error| error.to_string())
        }
    }

    pub fn set_directory_expanded(
        &self,
        session_id: String,
        directory: PathBuf,
        expanded: bool,
    ) -> Result<DirectoryTreeSnapshot, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let app = self.runtime.clone();
        let (snapshot, should_load) = self
            .files
            .set_directory_expanded(&session_id, &directory, expanded)
            .map_err(|error| error.to_string())?;
        let _ = app.publish(CoreEvent::DirectoryTreeUpdated(snapshot.clone()));
        if should_load {
            self.directory_tree_queue.enqueue(session_id, directory);
        }
        Ok(snapshot)
    }

    pub fn collapse_directory_tree(
        &self,
        session_id: String,
    ) -> Result<DirectoryTreeSnapshot, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let app = self.runtime.clone();
        let snapshot = self
            .files
            .collapse_directory_tree(&session_id)
            .map_err(|error| error.to_string())?;
        let _ = app.publish(CoreEvent::DirectoryTreeUpdated(snapshot.clone()));
        Ok(snapshot)
    }

    pub fn set_active_directory(
        &self,
        session_id: String,
        directory: PathBuf,
    ) -> Result<(), String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let files = self.files.clone();
        let library = self.library.clone();
        let (session_id, directory) = 'resolved: {
            let root = files
                .session_root(&session_id)
                .map_err(|error| error.to_string())?;
            if library
                .has_directory_snapshot(&root, &directory)
                .map_err(|error| error.to_string())?
            {
                break 'resolved Ok((session_id, directory));
            }
            match files.session_directory(&session_id, Some(&directory)) {
                Ok(directory) => Ok((session_id, directory)),
                // An unreachable NAS is not evidence that the saved directory was
                // deleted. Keep selection and let the browse request show its error.
                Err(crate::files::FsError::Io(error))
                    if error.kind() != std::io::ErrorKind::NotFound =>
                {
                    Ok((session_id, directory))
                }
                Err(error) => Err(error.to_string()),
            }
        }?;
        self.metadata_queue
            .clear_pending_outside_directory(&directory);
        self.preview_queue
            .clear_pending_outside_directory(&directory);
        self.directory_tree_queue.set_active(session_id, directory);
        Ok(())
    }

    pub fn search_directories(
        &self,
        session_id: String,
        search: String,
    ) -> Result<Option<Vec<DirectorySearchMatch>>, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let files = self.files.clone();
        let library = self.library.clone();
        {
            let root = files
                .session_root(&session_id)
                .map_err(|error| error.to_string())?;
            library
                .search_directories(&root, &search)
                .map_err(|error| error.to_string())
        }
    }

    pub fn refresh_directory(
        &self,
        session_id: String,
        directory: Option<PathBuf>,
    ) -> Result<DirectoryTreeSnapshot, String> {
        if self.runtime.is_stopping() {
            return Err("application is shutting down".into());
        }
        let files = self.files.clone();
        let library = self.library.clone();
        let (tree, root, resolved_directory) = {
            let root = files
                .session_root(&session_id)
                .map_err(|error| error.to_string())?;
            let resolved_directory = files
                .session_directory(&session_id, directory.as_deref())
                .map_err(|error| error.to_string())?;
            library
                .invalidate_directory_snapshots(&resolved_directory)
                .map_err(|error| error.to_string())?;
            let tree = files
                .refresh_directory(&session_id, directory.as_deref())
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((tree, root, resolved_directory))
        }?;
        self.projections
            .invalidate_resource_projections(&resolved_directory)
            .map_err(|error| error.to_string())?;
        self.metadata_queue
            .invalidate_directory(&resolved_directory);
        self.preview_queue.invalidate_directory(&resolved_directory);
        self.library
            .invalidate_index(&root)
            .map_err(|error| error.to_string())?;
        self.library_index_queue
            .schedule(self.runtime.clone(), root.clone());
        self.directory_tree_queue
            .enqueue(tree.session_id.clone(), root);
        Ok(tree)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppPaths, OxyApp};

    fn host(directory: &std::path::Path) -> OxyApp {
        OxyApp::open(AppPaths {
            data_dir: directory.join("state"),
            preview_dir: directory.join("previews"),
        })
        .unwrap()
        .0
    }

    fn photo(path: &std::path::Path) {
        image::RgbImage::new(8, 6).save(path).unwrap();
    }

    #[test]
    fn headless_folder_open_pages_only_direct_photos() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("photos");
        std::fs::create_dir_all(folder.join("child")).unwrap();
        photo(&folder.join("a.jpg"));
        photo(&folder.join("b.jpg"));
        photo(&folder.join("child/nested.jpg"));
        let app = host(directory.path());
        let session = app.browse.open_folder(folder).unwrap();
        let query = AssetQuery {
            page_size: Some(1),
            ..Default::default()
        };
        let first = app
            .browse
            .list_assets(session.id.clone(), None, query.clone(), None, None)
            .unwrap();
        assert_eq!(first.page.total, 2);
        assert_eq!(first.page.items[0].name, "a.jpg");
        assert_eq!(first.page.next_cursor, Some(1));
        let second = app
            .browse
            .list_assets(
                session.id,
                None,
                query,
                first.page.next_cursor,
                first.snapshot_revision,
            )
            .unwrap();
        assert_eq!(second.page.items[0].name, "b.jpg");
        assert!(second.page.next_cursor.is_none());
        assert!(app.shutdown(Duration::from_secs(2)));
    }

    #[test]
    fn headless_query_intersects_tag_and_metadata_filters() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("photos");
        std::fs::create_dir_all(&folder).unwrap();
        let selected = folder.join("selected.jpg");
        photo(&selected);
        let selected = selected.canonicalize().unwrap();
        photo(&folder.join("other.jpg"));
        let app = host(directory.path());
        let tag = app.tags.create_custom_tag(None, "selected").unwrap();
        app.tags
            .set_asset_tag(std::slice::from_ref(&selected), tag.id, true)
            .unwrap();
        app.metadata
            .patch_metadata(
                &selected,
                oxy_media::types::AssetKind::Jpeg,
                &crate::metadata::types::MetadataPatch {
                    rating: Some(Some(4)),
                    ..Default::default()
                },
            )
            .unwrap();
        let session = app.browse.open_folder(folder).unwrap();
        let page = app
            .browse
            .list_assets(
                session.id,
                None,
                AssetQuery {
                    tag_ids: vec![tag.id],
                    minimum_rating: Some(3),
                    ..Default::default()
                },
                None,
                None,
            )
            .unwrap();
        assert_eq!(page.page.total, 1);
        assert_eq!(page.page.items[0].name, "selected.jpg");
        assert_eq!(page.page.items[0].rating, Some(4));
        assert!(app.shutdown(Duration::from_secs(2)));
    }
}
