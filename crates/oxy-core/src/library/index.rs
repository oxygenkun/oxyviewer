//! Rebuildable asset and directory index.
//!
//! Derived from the volumes behind the explicit roots in [`crate::library::user::roots`].
//! Deleting every row here only costs a re-scan; no user-owned fact is stored
//! in these tables.

use crate::browse::types::{
    AssetQuery, AssetSummary, DirectorySearchMatch, DirectorySummary, Page,
};
use crate::library::repo::index::{
    self as repo, has_completed_directory_index, has_completed_index,
};
use crate::library::{
    DEFAULT_PAGE_SIZE, INDEX_WRITE_BATCH_SIZE, INDEX_WRITE_TIME_SLICE, Library, LibraryError,
    MAX_PAGE_SIZE,
};
use parking_lot::MutexGuard;
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

/// Drops every derived row for one root.
///
/// Removing a root is a user decision, but only the cache namespace may delete
/// cache rows. Callers in [`crate::library::user`] go through this function so a
/// cross-namespace delete shows up as a named call instead of raw SQL.
#[derive(Debug, Eq, PartialEq)]
struct IndexQueueEntry {
    depth: usize,
    sequence: usize,
    path: PathBuf,
}

impl Ord for IndexQueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap pops the greatest item. A smaller depth represents a
        // higher directory level and therefore a greater indexing priority.
        other
            .depth
            .cmp(&self.depth)
            .then_with(|| other.sequence.cmp(&self.sequence))
            .then_with(|| self.path.cmp(&other.path))
    }
}

impl PartialOrd for IndexQueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(crate) struct DirectoryPriorityQueue {
    root: PathBuf,
    pending: BinaryHeap<IndexQueueEntry>,
    visited: HashSet<PathBuf>,
    next_sequence: usize,
}

impl DirectoryPriorityQueue {
    fn new(root: PathBuf) -> Self {
        let pending = BinaryHeap::from([IndexQueueEntry {
            depth: 0,
            sequence: 0,
            path: root.clone(),
        }]);
        Self {
            pending,
            visited: HashSet::from([root.clone()]),
            root,
            next_sequence: 1,
        }
    }

    fn pop_next(&mut self) -> Option<(PathBuf, usize)> {
        self.pending.pop().map(|entry| (entry.path, entry.depth))
    }

    fn enqueue_children(
        &mut self,
        parent_depth: usize,
        directories: Vec<DirectorySummary>,
    ) -> Vec<DirectorySummary> {
        let mut safe_directories = directories
            .into_iter()
            .filter_map(|mut child| {
                let canonical = child.path.canonicalize().ok()?;
                if !canonical.starts_with(&self.root) || !self.visited.insert(canonical.clone()) {
                    return None;
                }
                child.path = canonical;
                Some(child)
            })
            .collect::<Vec<_>>();
        safe_directories.sort_unstable_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.path.cmp(&right.path))
        });
        for child in &safe_directories {
            self.pending.push(IndexQueueEntry {
                depth: parent_depth.saturating_add(1),
                sequence: self.next_sequence,
                path: child.path.clone(),
            });
            self.next_sequence = self.next_sequence.saturating_add(1);
        }
        safe_directories
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexStats {
    pub asset_count: usize,
    pub directory_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexProgress {
    pub root_path: PathBuf,
    pub current_directory: PathBuf,
    pub pending_directory_count: usize,
    pub asset_count: usize,
    pub directory_count: usize,
    pub stage: IndexStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexStage {
    Directories,
    Assets,
}

impl Library {
    /// Rebuilds one root in low-cost directory batches. Existing completed
    /// rows remain queryable until the final cleanup transaction, and only one
    /// worker per canonical root is admitted at a time.
    pub fn index_root(&self, root: &Path) -> Result<Option<IndexStats>, LibraryError> {
        self.index_root_with_progress(root, |_| {})
    }

    /// Returns whether a registered root has no complete index generation.
    /// Normal folder opens reuse a completed generation; explicit refresh
    /// invalidates it and makes this check true again.
    pub fn root_needs_index(&self, root: &Path) -> Result<bool, LibraryError> {
        let root = if self.roots()?.iter().any(|stored| stored == root) {
            root.to_owned()
        } else {
            root.canonicalize().unwrap_or_else(|_| root.to_owned())
        };
        let root = root.to_string_lossy();
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        let registered = crate::library::repo::contains_root(&connection, &root)?;
        Ok(registered && !has_completed_index(&connection, &root)?)
    }

    pub fn index_root_with_progress(
        &self,
        root: &Path,
        mut report_progress: impl FnMut(IndexProgress),
    ) -> Result<Option<IndexStats>, LibraryError> {
        self.index_root_with_progress_and_directory_scan(root, &mut report_progress, |directory| {
            crate::files::scan_index_directories_with_checkpoint(directory, || {
                self.foreground.wait_for_background();
            })
        })
    }

    /// Rebuilds an index while allowing the application-owned filesystem
    /// catalog to share immediate-child directory scans with its tree worker.
    pub fn index_root_with_progress_and_directory_scan(
        &self,
        root: &Path,
        mut report_progress: impl FnMut(IndexProgress),
        mut scan_directories: impl FnMut(&Path) -> Result<Vec<DirectorySummary>, crate::files::FsError>,
    ) -> Result<Option<IndexStats>, LibraryError> {
        let root = root.canonicalize()?;
        {
            let mut active = self.indexing_roots.lock();
            if !active.insert(root.clone()) {
                return Ok(None);
            }
        }
        let _gate = self.index_gate.lock();
        let result = self.index_root_inner(&root, &mut report_progress, &mut scan_directories);
        self.indexing_roots.lock().remove(&root);
        result.map(Some)
    }

    pub fn list_assets(
        &self,
        root: &Path,
        directory: &Path,
        query: &AssetQuery,
        offset: usize,
    ) -> Result<Option<Page<AssetSummary>>, LibraryError> {
        // The index stores ratings, colors, and flags but the paged query path
        // here only serves cheap filters; metadata filters fall back to a
        // scanned-and-enriched page.
        if query.needs_metadata_enrichment() || !query.tag_ids.is_empty() {
            return Ok(None);
        }
        let root = root.to_string_lossy();
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        if !has_completed_index(&connection, &root)? {
            return Ok(None);
        }
        let page_size = query
            .page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let (total, items) = repo::asset_page(
            &connection,
            repo::AssetPageRequest {
                root: &root,
                directory: &directory.to_string_lossy(),
                search,
                kind: query.kind,
                direction: query.direction,
                sort: query.sort,
                limit: page_size,
                offset,
            },
        )?;
        let end = offset.saturating_add(items.len());
        Ok(Some(Page {
            items,
            next_cursor: (end < total).then_some(end),
            total,
        }))
    }

    pub fn list_directories(
        &self,
        root: &Path,
        parent: &Path,
    ) -> Result<Option<Vec<DirectorySummary>>, LibraryError> {
        let root = root.to_string_lossy();
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        if !has_completed_directory_index(&connection, &root)? {
            return Ok(None);
        }
        let directories = repo::list_directories(&connection, &root, parent)?;
        Ok(Some(directories))
    }

    pub fn search_directories(
        &self,
        root: &Path,
        search: &str,
    ) -> Result<Option<Vec<DirectorySearchMatch>>, LibraryError> {
        let search = search.trim();
        if search.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let root_text = root.to_string_lossy();
        let mut reader = self.read_connection();
        let connection = reader.transaction()?;
        if !has_completed_directory_index(&connection, &root_text)? {
            return Ok(None);
        }
        let directories = repo::search_directories(&connection, &root_text, search)?;
        Ok(Some(
            directories
                .into_iter()
                .map(|directory| DirectorySearchMatch {
                    ancestors: directory_ancestors(root, &directory.path),
                    directory,
                })
                .collect(),
        ))
    }

    pub fn invalidate_index(&self, root: &Path) -> Result<(), LibraryError> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        repo::forget_asset_completion(&transaction, &root)?;
        repo::forget_directory_completion(&transaction, &root)?;
        transaction.commit()?;
        Ok(())
    }

    fn index_root_inner(
        &self,
        root: &Path,
        report_progress: &mut impl FnMut(IndexProgress),
        scan_directories: &mut impl FnMut(&Path) -> Result<Vec<DirectorySummary>, crate::files::FsError>,
    ) -> Result<IndexStats, LibraryError> {
        let index_started = Instant::now();
        let scan_id = self.next_scan_id()?;
        let mut queue = DirectoryPriorityQueue::new(root.to_owned());
        let mut discovered_directories = Vec::new();
        let mut asset_count = 0;
        let mut directory_count = 0;

        // Record the complete directory tree first. Besides making folder
        // discovery the highest-priority work, this avoids touching asset
        // metadata in a large directory until every folder is known.
        while let Some((directory, depth)) = queue.pop_next() {
            report_progress(IndexProgress {
                root_path: root.to_owned(),
                current_directory: directory.clone(),
                pending_directory_count: queue.pending.len(),
                asset_count,
                directory_count,
                stage: IndexStage::Directories,
            });
            if self.foreground.is_stopped() || !self.contains_root(root)? {
                return Ok(IndexStats {
                    asset_count,
                    directory_count,
                });
            }
            // Preserve the last completed generation if any directory becomes
            // unreadable. Publishing a partial scan as complete would turn a
            // transient permission or volume error into false deletions.
            self.foreground.wait_for_background();
            let directories = scan_directories(&directory)?;
            let safe_directories = queue.enqueue_children(depth, directories);
            directory_count += safe_directories.len();
            self.write_directory_index_batch(root, &directory, scan_id, &safe_directories)?;
            discovered_directories.push(directory.clone());
            report_progress(IndexProgress {
                root_path: root.to_owned(),
                current_directory: directory,
                pending_directory_count: queue.pending.len(),
                asset_count,
                directory_count,
                stage: IndexStage::Directories,
            });
        }

        if !self.finish_directory_index(root, scan_id, directory_count)? {
            return Ok(IndexStats {
                asset_count,
                directory_count,
            });
        }

        for (index, directory) in discovered_directories.iter().enumerate() {
            report_progress(IndexProgress {
                root_path: root.to_owned(),
                current_directory: directory.clone(),
                pending_directory_count: discovered_directories.len() - index,
                asset_count,
                directory_count,
                stage: IndexStage::Assets,
            });
            if self.foreground.is_stopped() || !self.contains_root(root)? {
                return Ok(IndexStats {
                    asset_count,
                    directory_count,
                });
            }
            self.foreground.wait_for_background();
            let assets = if let Some(assets) =
                self.snapshots
                    .validated_directory_assets(root, directory, index_started)
            {
                assets
            } else {
                Arc::new(crate::files::scan_assets_with_progress(directory, |_| {
                    self.foreground.wait_for_background();
                })?)
            };
            asset_count += assets.len();
            self.write_asset_index_batch(root, directory, scan_id, assets.as_ref())?;
            report_progress(IndexProgress {
                root_path: root.to_owned(),
                current_directory: directory.clone(),
                pending_directory_count: discovered_directories.len() - index - 1,
                asset_count,
                directory_count,
                stage: IndexStage::Assets,
            });
        }
        if !self.foreground.is_stopped() {
            self.finish_index(root, scan_id, asset_count, directory_count)?;
        }
        Ok(IndexStats {
            asset_count,
            directory_count,
        })
    }

    fn write_directory_index_batch(
        &self,
        root: &Path,
        parent: &Path,
        scan_id: i64,
        directories: &[DirectorySummary],
    ) -> Result<(), LibraryError> {
        {
            let mut connection = self.write();
            let transaction = connection.transaction()?;
            if !crate::library::repo::contains_root(&transaction, &root.to_string_lossy())? {
                return Ok(());
            }
            // The parent was inserted optimistically when its own parent was
            // scanned. Once this level has been visited, its child status is
            // authoritative and can be corrected without another filesystem read.
            repo::update_directory_status(
                &transaction,
                root,
                parent,
                !directories.is_empty(),
                scan_id,
            )?;
            transaction.commit()?;
            // Hand the writer to an existing waiter before starting another batch.
            MutexGuard::unlock_fair(connection);
        }

        let mut offset = 0;
        while offset < directories.len() {
            let batch =
                &directories[offset..directories.len().min(offset + INDEX_WRITE_BATCH_SIZE)];
            let mut connection = self.write();
            let started = Instant::now();
            let transaction = connection.transaction()?;
            if !crate::library::repo::contains_root(&transaction, &root.to_string_lossy())? {
                return Ok(());
            }
            {
                let mut statement = repo::directory_insert(&transaction)?;
                for (processed, directory) in batch.iter().enumerate() {
                    if processed > 0 && started.elapsed() >= INDEX_WRITE_TIME_SLICE {
                        break;
                    }
                    offset += 1;
                    repo::write_directory(&mut statement, root, parent, directory, scan_id)?;
                }
            }
            transaction.commit()?;
            // Hand the writer to an existing waiter before starting another batch.
            MutexGuard::unlock_fair(connection);
        }
        Ok(())
    }

    pub(crate) fn write_asset_index_batch(
        &self,
        root: &Path,
        parent: &Path,
        scan_id: i64,
        assets: &[AssetSummary],
    ) -> Result<(), LibraryError> {
        let mut offset = 0;
        while offset < assets.len() {
            let batch = &assets[offset..assets.len().min(offset + INDEX_WRITE_BATCH_SIZE)];
            let mut connection = self.write();
            let started = Instant::now();
            let transaction = connection.transaction()?;
            if !crate::library::repo::contains_root(&transaction, &root.to_string_lossy())? {
                return Ok(());
            }
            {
                let mut statements = repo::AssetStatements::new(&transaction)?;
                for (processed, asset) in batch.iter().enumerate() {
                    if processed > 0 && started.elapsed() >= INDEX_WRITE_TIME_SLICE {
                        break;
                    }
                    offset += 1;
                    if statements.mark_unchanged(root, parent, scan_id, asset)? {
                        continue;
                    }
                    statements.upsert(root, parent, scan_id, asset)?;
                    statements.insert_search_key(root, asset)?;
                    let search_rowid = statements.search_rowid(root, asset)?;
                    statements.insert_search(root, parent, asset, search_rowid)?;
                }
            }
            transaction.commit()?;
            // Hand the writer to an existing waiter before starting another batch.
            MutexGuard::unlock_fair(connection);
        }
        Ok(())
    }

    pub(crate) fn next_scan_id(&self) -> Result<i64, LibraryError> {
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        let scan_id = repo::read_scan_id(&transaction)?;
        repo::advance_scan_id(&transaction)?;
        transaction.commit()?;
        Ok(scan_id)
    }

    fn finish_directory_index(
        &self,
        root: &Path,
        scan_id: i64,
        directory_count: usize,
    ) -> Result<bool, LibraryError> {
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        if !crate::library::repo::contains_root(&transaction, &root.to_string_lossy())? {
            return Ok(false);
        }
        repo::forget_old_directories(&transaction, root, scan_id)?;
        repo::complete_directory_index(&transaction, root, directory_count)?;
        transaction.commit()?;
        Ok(true)
    }

    pub(crate) fn finish_index(
        &self,
        root: &Path,
        scan_id: i64,
        asset_count: usize,
        directory_count: usize,
    ) -> Result<(), LibraryError> {
        let mut connection = self.write();
        let transaction = connection.transaction()?;
        if !crate::library::repo::contains_root(&transaction, &root.to_string_lossy())? {
            return Ok(());
        }
        repo::forget_old_search_rows(&transaction, root, scan_id)?;
        repo::forget_old_search_keys(&transaction, root, scan_id)?;
        repo::forget_old_assets(&transaction, root, scan_id)?;
        repo::complete_asset_index(&transaction, root, asset_count, directory_count)?;
        transaction.commit()?;
        Ok(())
    }
}

fn directory_ancestors(root: &Path, directory: &Path) -> Vec<DirectorySummary> {
    let Ok(relative) = directory.strip_prefix(root) else {
        return Vec::new();
    };
    let components = relative.components().collect::<Vec<_>>();
    let mut path = root.to_path_buf();
    components
        .iter()
        .take(components.len().saturating_sub(1))
        .map(|component| {
            path.push(component.as_os_str());
            DirectorySummary {
                path: path.clone(),
                name: component.as_os_str().to_string_lossy().into_owned(),
                has_children: true,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn directory_priority_queue_visits_all_siblings_before_descendants() {
        let root = tempdir().unwrap();
        for path in ["beta/deep", "alpha/deep"] {
            std::fs::create_dir_all(root.path().join(path)).unwrap();
        }
        let canonical_root = root.path().canonicalize().unwrap();
        let mut queue = DirectoryPriorityQueue::new(canonical_root.clone());
        let mut order = Vec::new();

        while let Some((directory, depth)) = queue.pop_next() {
            order.push(directory.strip_prefix(&canonical_root).unwrap().to_owned());
            let scan = crate::files::scan_index_directory(&directory).unwrap();
            queue.enqueue_children(depth, scan.directories);
        }

        assert_eq!(
            order,
            ["", "alpha", "beta", "alpha/deep", "beta/deep"].map(PathBuf::from)
        );
    }

    #[test]
    fn higher_level_directory_preempts_an_earlier_deep_directory() {
        let root = tempdir().unwrap();
        let shallow = root.path().join("shallow");
        let deep = root.path().join("parent").join("deep");
        std::fs::create_dir_all(&shallow).unwrap();
        std::fs::create_dir_all(&deep).unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        let mut queue = DirectoryPriorityQueue::new(canonical_root);
        queue.pop_next();

        queue.enqueue_children(
            1,
            vec![DirectorySummary {
                path: deep.canonicalize().unwrap(),
                name: "deep".into(),
                has_children: false,
            }],
        );
        queue.enqueue_children(
            0,
            vec![DirectorySummary {
                path: shallow.canonicalize().unwrap(),
                name: "shallow".into(),
                has_children: false,
            }],
        );

        assert_eq!(queue.pop_next(), Some((shallow.canonicalize().unwrap(), 1)));
        assert_eq!(queue.pop_next(), Some((deep.canonicalize().unwrap(), 2)));
    }
}
