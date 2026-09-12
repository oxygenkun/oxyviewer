use oxy_domain::{
    AssetKind, AssetQuery, AssetSort, AssetSummary, DirectorySummary, DirectoryTreeNode,
    DirectoryTreeSnapshot, FileDeletionMode, FileOperation, FileOperationResult, FolderSession,
    Page, SortDirection,
};
use parking_lot::{Mutex, RwLock};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

#[cfg(target_os = "macos")]
mod bulk_attributes;
mod identity;

pub use identity::{FileObservation, observe_file};

const DEFAULT_PAGE_SIZE: usize = 250;
const MAX_PAGE_SIZE: usize = 1_000;

#[derive(Debug, Error)]
pub enum FsError {
    #[error("folder does not exist or is not a directory: {0}")]
    InvalidFolder(PathBuf),
    #[error("folder session was not found: {0}")]
    SessionNotFound(String),
    #[error("path is outside the opened folder: {0}")]
    OutsideSessionRoot(PathBuf),
    #[error("unsupported or invalid file name")]
    InvalidFileName,
    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("failed to move item to the system trash: {0}")]
    Trash(String),
    #[error("permanent deletion is only available for paths without system trash support: {0}")]
    PermanentDeleteUnsupported(PathBuf),
}

#[derive(Default)]
pub struct FsCatalog {
    sessions: RwLock<HashMap<String, PathBuf>>,
    asset_cache: RwLock<HashMap<PathBuf, Arc<Vec<AssetSummary>>>>,
    directory_cache: RwLock<HashMap<PathBuf, Arc<Vec<DirectorySummary>>>>,
    directory_trees: RwLock<HashMap<String, Arc<Mutex<DirectoryTreeState>>>>,
    next_directory_tree_revision: AtomicU64,
}

#[derive(Debug)]
struct DirectoryTreeState {
    snapshot: DirectoryTreeSnapshot,
    loading_directories: HashSet<PathBuf>,
    scan_generation: u64,
    scanned_directories: HashSet<PathBuf>,
}

/// One cheap, non-recursive filesystem batch used by the background library
/// indexer. It deliberately contains no decoded image or embedded metadata.
#[derive(Debug)]
pub struct DirectoryScan {
    pub assets: Vec<AssetSummary>,
    pub directories: Vec<DirectorySummary>,
}

impl FsCatalog {
    pub fn open_folder(&self, path: impl AsRef<Path>) -> Result<FolderSession, FsError> {
        let root_path = path.as_ref().canonicalize()?;
        if !root_path.is_dir() {
            return Err(FsError::InvalidFolder(root_path));
        }

        Ok(self.restore_registered_folder(root_path))
    }

    /// The caller must obtain this canonical root from the registered library.
    /// Restoring a trusted session must not stat an offline network volume.
    pub fn restore_registered_folder(&self, root_path: PathBuf) -> FolderSession {
        let opened_at_ms = epoch_ms(SystemTime::now());
        let id = stable_id(&(root_path.clone(), opened_at_ms));
        self.sessions.write().insert(id.clone(), root_path.clone());

        let display_name = root_path
            .file_name()
            .and_then(|name| name.to_str())
            .map_or_else(|| root_path.to_string_lossy().into_owned(), str::to_owned);

        let session = FolderSession {
            id,
            display_name,
            deletion_mode: deletion_mode_for_path(&root_path),
            root_path,
            opened_at_ms,
        };
        self.directory_trees.write().insert(
            session.id.clone(),
            Arc::new(Mutex::new(DirectoryTreeState {
                snapshot: DirectoryTreeSnapshot {
                    session_id: session.id.clone(),
                    revision: self.next_tree_revision(),
                    root: DirectoryTreeNode {
                        entry: DirectorySummary {
                            path: session.root_path.clone(),
                            name: session.display_name.clone(),
                            has_children: true,
                        },
                        expanded: false,
                        children: None,
                    },
                },
                loading_directories: HashSet::new(),
                scan_generation: 0,
                scanned_directories: HashSet::new(),
            })),
        );

        session
    }

    pub fn list_assets(
        &self,
        session_id: &str,
        directory: Option<&Path>,
        query: &AssetQuery,
        cursor: Option<usize>,
    ) -> Result<Page<AssetSummary>, FsError> {
        let directory = self.resolve_session_directory(session_id, directory)?;
        let assets = self.cached_assets(&directory)?;
        Ok(page_assets(&assets, query, cursor.unwrap_or(0)))
    }

    /// Returns the cheap, non-recursive summaries before paging. Metadata-aware
    /// callers use this only when a metadata filter explicitly requires a full
    /// directory pass.
    pub fn list_asset_candidates(
        &self,
        session_id: &str,
        directory: Option<&Path>,
    ) -> Result<Vec<AssetSummary>, FsError> {
        let directory = self.resolve_session_directory(session_id, directory)?;
        Ok(self.cached_assets(&directory)?.as_ref().clone())
    }

    pub fn list_directories(
        &self,
        session_id: &str,
        directory: Option<&Path>,
    ) -> Result<Vec<DirectorySummary>, FsError> {
        let directory = self.resolve_session_directory(session_id, directory)?;
        let directories = self.cached_directories(&directory)?;
        Ok(directories.as_ref().clone())
    }

    /// Returns the Rust-owned, lazily loaded directory tree projection.
    /// Background discovery can populate collapsed levels without expanding them.
    pub fn directory_tree(&self, session_id: &str) -> Result<DirectoryTreeSnapshot, FsError> {
        let tree = self.directory_tree_state(session_id)?;
        let state = tree.lock();
        Ok(state.snapshot.clone())
    }

    /// Resolves a UI activity signal against the Rust-owned tree without
    /// touching the filesystem. Clicked nodes were originally supplied by
    /// this projection, so the stored path is already canonical and scoped.
    pub fn known_directory_tree_path(
        &self,
        session_id: &str,
        directory: &Path,
    ) -> Result<PathBuf, FsError> {
        let tree = self.directory_tree_state(session_id)?;
        let state = tree.lock();
        find_tree_node(&state.snapshot.root, directory)
            .map(|node| node.entry.path.clone())
            .ok_or_else(|| FsError::InvalidFolder(directory.to_owned()))
    }

    /// Records an expansion intent without performing filesystem IO. The
    /// boolean tells the runtime whether it won the right to load this level.
    pub fn set_directory_expanded(
        &self,
        session_id: &str,
        directory: &Path,
        expanded: bool,
    ) -> Result<(DirectoryTreeSnapshot, bool), FsError> {
        let directory = self.known_directory_tree_path(session_id, directory)?;
        let tree = self.directory_tree_state(session_id)?;
        let mut state = tree.lock();
        let needs_load = {
            let node = find_tree_node_mut(&mut state.snapshot.root, &directory)
                .ok_or_else(|| FsError::InvalidFolder(directory.clone()))?;
            let known_leaf = node.children.as_ref().is_some_and(Vec::is_empty);
            node.expanded = expanded && !known_leaf;
            node.expanded && node.children.is_none()
        };
        if !expanded {
            state.loading_directories.remove(&directory);
        }
        let should_load = needs_load && state.loading_directories.insert(directory);
        state.snapshot.revision = self.next_tree_revision();
        Ok((state.snapshot.clone(), should_load))
    }

    /// Collapses every expanded level without performing filesystem IO.
    /// Loaded children stay cached so re-expansion remains cheap.
    pub fn collapse_directory_tree(
        &self,
        session_id: &str,
    ) -> Result<DirectoryTreeSnapshot, FsError> {
        let tree = self.directory_tree_state(session_id)?;
        let mut state = tree.lock();
        collapse_tree_nodes(&mut state.snapshot.root);
        state.loading_directories.clear();
        state.snapshot.revision = self.next_tree_revision();
        Ok(state.snapshot.clone())
    }

    /// Completes one previously admitted expansion. A collapse or refresh can
    /// revoke the admission while IO is in flight, preventing stale results
    /// from replacing newer tree state.
    pub fn load_directory_children(
        &self,
        session_id: &str,
        directory: &Path,
    ) -> Result<DirectoryTreeSnapshot, FsError> {
        let requested_directory = directory.to_owned();
        let tree = self.directory_tree_state(session_id)?;
        let generation = {
            let state = tree.lock();
            if !state.loading_directories.contains(&requested_directory) {
                return Ok(state.snapshot.clone());
            }
            state.scan_generation
        };
        let directory = match self.resolve_session_directory(session_id, Some(directory)) {
            Ok(directory) => directory,
            Err(error) => {
                self.fail_directory_load(session_id, &requested_directory, generation)?;
                return Err(error);
            }
        };
        let directories = match list_directories(&directory) {
            Ok(directories) => directories,
            Err(error) => {
                self.fail_directory_load(session_id, &directory, generation)?;
                return Err(error);
            }
        };
        let mut state = tree.lock();
        if state.scan_generation != generation || !state.loading_directories.remove(&directory) {
            return Ok(state.snapshot.clone());
        }
        let node = find_tree_node_mut(&mut state.snapshot.root, &directory)
            .ok_or_else(|| FsError::InvalidFolder(directory.clone()))?;
        replace_tree_children(node, &directories);
        state.scanned_directories.insert(directory);
        state.snapshot.revision = self.next_tree_revision();
        Ok(state.snapshot.clone())
    }

    /// Discovers a collapsed level in the background without changing expansion
    /// intent. Symlink aliases are not traversed, avoiding cycles and escapes.
    pub fn prefetch_directory_children(
        &self,
        session_id: &str,
        directory: &Path,
    ) -> Result<(DirectoryTreeSnapshot, Vec<PathBuf>), FsError> {
        self.prefetch_directory_children_with(session_id, directory, list_directories)
    }

    fn prefetch_directory_children_with(
        &self,
        session_id: &str,
        directory: &Path,
        scan: impl FnOnce(&Path) -> Result<Vec<DirectorySummary>, FsError>,
    ) -> Result<(DirectoryTreeSnapshot, Vec<PathBuf>), FsError> {
        let tree = self.directory_tree_state(session_id)?;
        let (needs_read, generation) = {
            let state = tree.lock();
            let node = find_tree_node(&state.snapshot.root, directory)
                .ok_or_else(|| FsError::InvalidFolder(directory.to_owned()))?;
            (
                node.children.is_none() || !state.scanned_directories.contains(directory),
                state.scan_generation,
            )
        };
        if needs_read {
            let resolved = self.resolve_session_directory(session_id, Some(directory))?;
            if resolved != directory {
                return Ok((tree.lock().snapshot.clone(), Vec::new()));
            }
            let directories = scan(directory)?;
            let mut state = tree.lock();
            if state.scan_generation != generation {
                return Ok((state.snapshot.clone(), Vec::new()));
            }
            // A foreground load or refresh may have won while IO was running.
            if !state.scanned_directories.contains(directory)
                && let Some(node) = find_tree_node_mut(&mut state.snapshot.root, directory)
            {
                replace_tree_children(node, &directories);
                state.scanned_directories.insert(directory.to_owned());
                state.snapshot.revision = self.next_tree_revision();
            }
        }
        let state = tree.lock();
        let children = find_tree_node(&state.snapshot.root, directory)
            .and_then(|node| node.children.as_ref())
            .into_iter()
            .flatten()
            .map(|child| child.entry.path.clone())
            .collect();
        Ok((state.snapshot.clone(), children))
    }

    pub fn refresh_directory(
        &self,
        session_id: &str,
        directory: Option<&Path>,
    ) -> Result<DirectoryTreeSnapshot, FsError> {
        let directory = self.resolve_session_directory(session_id, directory)?;
        let tree = self.directory_tree_state(session_id)?;
        // Keep the visible tree and expansion intent while background discovery
        // re-reads every level, including previously empty/collapsed nodes.
        let generation = {
            let mut state = tree.lock();
            state.scan_generation += 1;
            state.scanned_directories.clear();
            state.loading_directories.clear();
            state.scan_generation
        };
        self.asset_cache.write().remove(&directory);
        self.directory_cache.write().remove(&directory);
        let directories = list_directories(&directory)?;
        let mut state = tree.lock();
        if state.scan_generation != generation {
            return Ok(state.snapshot.clone());
        }
        if let Some(node) = find_tree_node_mut(&mut state.snapshot.root, &directory) {
            replace_tree_children(node, &directories);
            state.scanned_directories.insert(directory);
        }
        state.snapshot.revision = self.next_tree_revision();
        Ok(state.snapshot.clone())
    }

    pub fn invalidate_directory(&self, directory: &Path) {
        self.asset_cache.write().remove(directory);
        self.directory_cache.write().remove(directory);
    }

    pub fn get_asset(&self, path: impl AsRef<Path>) -> Result<AssetSummary, FsError> {
        summary_for_path(path.as_ref())?.ok_or(FsError::InvalidFileName)
    }

    pub fn session_root(&self, session_id: &str) -> Result<PathBuf, FsError> {
        self.sessions
            .read()
            .get(session_id)
            .cloned()
            .ok_or_else(|| FsError::SessionNotFound(session_id.to_owned()))
    }

    pub fn session_directory(
        &self,
        session_id: &str,
        directory: Option<&Path>,
    ) -> Result<PathBuf, FsError> {
        self.resolve_session_directory(session_id, directory)
    }

    fn resolve_session_directory(
        &self,
        session_id: &str,
        directory: Option<&Path>,
    ) -> Result<PathBuf, FsError> {
        let root = self
            .sessions
            .read()
            .get(session_id)
            .cloned()
            .ok_or_else(|| FsError::SessionNotFound(session_id.to_owned()))?;
        let directory = directory.unwrap_or(&root).canonicalize()?;
        if !directory.is_dir() {
            return Err(FsError::InvalidFolder(directory));
        }
        if !directory.starts_with(&root) {
            return Err(FsError::OutsideSessionRoot(directory));
        }
        Ok(directory)
    }

    fn cached_assets(&self, directory: &Path) -> Result<Arc<Vec<AssetSummary>>, FsError> {
        if let Some(assets) = self.asset_cache.read().get(directory).cloned() {
            return Ok(assets);
        }

        let assets = Arc::new(scan_assets(directory)?);
        let mut cache = self.asset_cache.write();
        Ok(cache
            .entry(directory.to_owned())
            .or_insert_with(|| assets.clone())
            .clone())
    }

    fn cached_directories(&self, directory: &Path) -> Result<Arc<Vec<DirectorySummary>>, FsError> {
        if let Some(directories) = self.directory_cache.read().get(directory).cloned() {
            return Ok(directories);
        }

        let directories = Arc::new(list_directories(directory)?);
        let mut cache = self.directory_cache.write();
        Ok(cache
            .entry(directory.to_owned())
            .or_insert_with(|| directories.clone())
            .clone())
    }

    fn next_tree_revision(&self) -> u64 {
        self.next_directory_tree_revision
            .fetch_add(1, AtomicOrdering::Relaxed)
            .saturating_add(1)
    }

    fn directory_tree_state(
        &self,
        session_id: &str,
    ) -> Result<Arc<Mutex<DirectoryTreeState>>, FsError> {
        self.directory_trees
            .read()
            .get(session_id)
            .cloned()
            .ok_or_else(|| FsError::SessionNotFound(session_id.to_owned()))
    }

    fn fail_directory_load(
        &self,
        session_id: &str,
        directory: &Path,
        generation: u64,
    ) -> Result<(), FsError> {
        let tree = self.directory_tree_state(session_id)?;
        let mut state = tree.lock();
        if state.scan_generation != generation {
            return Ok(());
        }
        state.loading_directories.remove(directory);
        if let Some(node) = find_tree_node_mut(&mut state.snapshot.root, directory) {
            node.expanded = false;
        }
        state.snapshot.revision = self.next_tree_revision();
        Ok(())
    }
}

fn find_tree_node_mut<'a>(
    node: &'a mut DirectoryTreeNode,
    path: &Path,
) -> Option<&'a mut DirectoryTreeNode> {
    if node.entry.path == path {
        return Some(node);
    }
    node.children
        .as_mut()?
        .iter_mut()
        .find_map(|child| find_tree_node_mut(child, path))
}

fn find_tree_node<'a>(node: &'a DirectoryTreeNode, path: &Path) -> Option<&'a DirectoryTreeNode> {
    if node.entry.path == path {
        return Some(node);
    }
    node.children
        .as_ref()?
        .iter()
        .find_map(|child| find_tree_node(child, path))
}

fn collapse_tree_nodes(node: &mut DirectoryTreeNode) {
    node.expanded = false;
    if let Some(children) = node.children.as_mut() {
        for child in children {
            collapse_tree_nodes(child);
        }
    }
}

fn replace_tree_children(node: &mut DirectoryTreeNode, directories: &[DirectorySummary]) {
    let previous = node
        .children
        .take()
        .unwrap_or_default()
        .into_iter()
        .map(|child| (child.entry.path.clone(), child))
        .collect::<HashMap<_, _>>();
    let mut previous = previous;
    let children = directories
        .iter()
        .cloned()
        .map(|entry| {
            if let Some(mut child) = previous.remove(&entry.path) {
                child.entry.name = entry.name;
                child.entry.has_children = child
                    .children
                    .as_ref()
                    .map_or(entry.has_children, |children| !children.is_empty());
                child
            } else {
                DirectoryTreeNode {
                    entry,
                    expanded: false,
                    children: None,
                }
            }
        })
        .collect::<Vec<_>>();
    node.entry.has_children = !children.is_empty();
    if children.is_empty() {
        node.expanded = false;
    }
    node.children = Some(children);
}

pub fn scan_index_directory(root: &Path) -> Result<DirectoryScan, FsError> {
    if !root.is_dir() {
        return Err(FsError::InvalidFolder(root.to_owned()));
    }
    let mut assets = Vec::new();
    let mut directories = Vec::new();
    for entry in fs::read_dir(root)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_dir() {
            let Some(name) = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            directories.push(DirectorySummary {
                path,
                name,
                has_children: true,
            });
        } else if let Some(summary) = summary_for_path(&path)? {
            assets.push(summary);
        }
    }
    Ok(DirectoryScan {
        assets,
        directories,
    })
}

/// Discovers only the immediate child directories needed by the first phase of
/// library indexing. Asset metadata is intentionally left untouched until the
/// directory tree has been recorded.
pub fn scan_index_directories(root: &Path) -> Result<Vec<DirectorySummary>, FsError> {
    scan_index_directories_with_checkpoint(root, || {})
}

pub fn scan_index_directories_with_checkpoint(
    root: &Path,
    mut checkpoint: impl FnMut(),
) -> Result<Vec<DirectorySummary>, FsError> {
    checkpoint();
    if !root.is_dir() {
        return Err(FsError::InvalidFolder(root.to_owned()));
    }
    let mut directories = Vec::new();
    for entry in fs::read_dir(root)? {
        checkpoint();
        let entry = entry?;
        let path = entry.path();
        let is_directory = entry.file_type().map_or_else(
            |_| path.is_dir(),
            |file_type| file_type.is_dir() || file_type.is_symlink() && path.is_dir(),
        );
        if !is_directory {
            continue;
        }
        let Some(name) = path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        directories.push(DirectorySummary {
            path,
            name,
            has_children: true,
        });
    }
    Ok(directories)
}

/// Discovers only the immediate assets for the second phase of library
/// indexing, after the complete directory tree has been recorded.
pub fn scan_index_assets(root: &Path) -> Result<Vec<AssetSummary>, FsError> {
    scan_assets_with_progress(root, |_| {})
}

pub fn list_directories(root: &Path) -> Result<Vec<DirectorySummary>, FsError> {
    let mut directories = scan_index_directories(root)?;
    directories.sort_unstable_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(directories)
}

pub fn scan_directory(
    root: &Path,
    query: &AssetQuery,
    offset: usize,
) -> Result<Page<AssetSummary>, FsError> {
    if !root.is_dir() {
        return Err(FsError::InvalidFolder(root.to_owned()));
    }

    let assets = scan_assets(root)?;
    Ok(page_assets(&assets, query, offset))
}

fn scan_assets(root: &Path) -> Result<Vec<AssetSummary>, FsError> {
    scan_assets_with_progress(root, |_| {})
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScanProgress {
    pub resolving: bool,
    pub resolve_ms: u64,
    pub discovered_count: usize,
    pub reading_attributes: bool,
    pub enumeration_ms: u64,
    pub attributes_ms: u64,
}

const SCAN_PROGRESS_INTERVAL: usize = 256;

/// One enumeration supplies file types and sidecar pairing. On Windows,
/// DirEntry::metadata reuses enumeration attributes instead of a path stat.
/// Keep size/mtime accurate because sorting and preview cache keys need them.
pub fn scan_assets_with_progress(
    root: &Path,
    report: impl FnMut(ScanProgress),
) -> Result<Vec<AssetSummary>, FsError> {
    #[cfg(target_os = "macos")]
    let mut report = report;
    #[cfg(target_os = "macos")]
    if let Some(assets) = bulk_attributes::scan(root, &mut report)? {
        return Ok(assets);
    }
    scan_assets_portable(root, report)
}

fn scan_assets_portable(
    root: &Path,
    mut report: impl FnMut(ScanProgress),
) -> Result<Vec<AssetSummary>, FsError> {
    let started = Instant::now();
    let mut progress = ScanProgress::default();
    report(progress);
    let mut entries = Vec::new();
    let mut sidecars = HashSet::new();
    for (entry_index, entry) in fs::read_dir(root)?.enumerate() {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            continue;
        }
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("xmp"))
            && (file_type.is_file() || path.is_file())
        {
            sidecars.insert(sidecar_key(&path));
        }
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(AssetKind::from_extension)
            .is_some()
        {
            entries.push((entry, file_type.is_symlink()));
            progress.discovered_count += 1;
        }
        if (entry_index + 1) % SCAN_PROGRESS_INTERVAL == 0 {
            progress.enumeration_ms = started.elapsed().as_millis() as u64;
            report(progress);
        }
    }
    progress.enumeration_ms = started.elapsed().as_millis() as u64;
    progress.reading_attributes = true;
    report(progress);
    let attributes_started = Instant::now();
    let mut assets = Vec::with_capacity(entries.len());
    for (entry_index, (entry, is_symlink)) in entries.into_iter().enumerate() {
        let path = entry.path();
        let metadata = if is_symlink {
            fs::metadata(&path)
        } else {
            entry.metadata()
        };
        let metadata = match metadata {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if metadata.is_file() {
            let has_sidecar = sidecars.contains(&sidecar_key(&sidecar_path(&path)));
            if let Some(summary) = summary_from_metadata(&path, &metadata, has_sidecar)? {
                assets.push(summary);
            }
        }
        if (entry_index + 1) % SCAN_PROGRESS_INTERVAL == 0 {
            progress.attributes_ms = attributes_started.elapsed().as_millis() as u64;
            report(progress);
        }
    }
    progress.discovered_count = assets.len();
    progress.attributes_ms = attributes_started.elapsed().as_millis() as u64;
    report(progress);
    Ok(assets)
}

fn sidecar_key(path: &Path) -> PathBuf {
    // Sidecar extension matching is portable and case-insensitive, while the
    // stem keeps the filesystem's native case semantics on Unix.
    let extension_normalized = path.with_extension("xmp");
    if cfg!(windows) {
        PathBuf::from(extension_normalized.to_string_lossy().to_lowercase())
    } else {
        extension_normalized
    }
}

pub fn page_assets(
    assets: &[AssetSummary],
    query: &AssetQuery,
    offset: usize,
) -> Page<AssetSummary> {
    let search = query.search.as_deref().map(str::to_lowercase);
    let mut items = Vec::new();
    for summary in assets {
        if query.kind.is_some_and(|kind| kind != summary.kind) {
            continue;
        }
        if search
            .as_ref()
            .is_some_and(|needle| !summary.name.to_lowercase().contains(needle))
        {
            continue;
        }
        if query
            .minimum_rating
            .is_some_and(|minimum| summary.rating.unwrap_or_default() < minimum)
        {
            continue;
        }
        if !query.color_labels.is_empty()
            && summary.color_label.as_deref().is_none_or(|value| {
                !query
                    .color_labels
                    .iter()
                    .any(|label| value.eq_ignore_ascii_case(label))
            })
        {
            continue;
        }
        items.push(summary);
    }

    if matches!(query.sort, AssetSort::Name) {
        let mut keyed = items
            .into_iter()
            .map(|summary| (summary.name.to_ascii_lowercase(), summary))
            .collect::<Vec<_>>();
        keyed.sort_unstable_by(|(left_key, left), (right_key, right)| {
            left_key
                .cmp(right_key)
                .then_with(|| left.name.cmp(&right.name))
        });
        items = keyed.into_iter().map(|(_, summary)| summary).collect();
    } else {
        items.sort_unstable_by(|left, right| compare_assets(left, right, query.sort));
    }
    if matches!(query.direction, SortDirection::Descending) {
        items.reverse();
    }

    let total = items.len();
    let page_size = query
        .page_size
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let end = offset.saturating_add(page_size).min(total);
    let page_items = if offset < total {
        items.drain(offset..end).cloned().collect()
    } else {
        Vec::new()
    };

    Page {
        items: page_items,
        next_cursor: (end < total).then_some(end),
        total,
    }
}

pub fn execute_file_operation(operation: &FileOperation) -> Result<FileOperationResult, FsError> {
    match operation {
        FileOperation::Rename { source, new_name } => rename_asset(source, new_name),
        FileOperation::Copy {
            sources,
            destination_dir,
        } => transfer_assets(sources, destination_dir, false),
        FileOperation::Move {
            sources,
            destination_dir,
        } => transfer_assets(sources, destination_dir, true),
        FileOperation::Trash { paths } => trash_assets(paths),
        FileOperation::DeletePermanently { paths } => delete_assets_permanently(paths),
    }
}

pub fn deletion_mode_for_path(path: &Path) -> FileDeletionMode {
    #[cfg(target_os = "macos")]
    if !macos_volume_is_local(path) {
        return FileDeletionMode::Permanent;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::{Win32::Storage::FileSystem::GetDriveTypeW, core::PCWSTR};

        let Some(root) = windows_volume_root(path) else {
            return FileDeletionMode::Trash;
        };
        let mut root = root.as_os_str().encode_wide().collect::<Vec<_>>();
        root.push(0);
        // GetDriveTypeW only reads the locally known drive mapping. DRIVE_REMOTE
        // is 4 and covers both UNC shares and mapped SMB drive letters.
        if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } == 4 {
            return FileDeletionMode::Permanent;
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = path;

    FileDeletionMode::Trash
}

#[cfg(target_os = "macos")]
fn macos_volume_is_local(path: &Path) -> bool {
    use std::{ffi::CString, mem::MaybeUninit, os::unix::ffi::OsStrExt};

    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut stats = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `stats` points to writable, properly
    // aligned storage which is only assumed initialized after statfs succeeds.
    if unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: A successful statfs call initialized the complete structure.
    let stats = unsafe { stats.assume_init() };
    stats.f_flags & libc::MNT_LOCAL as u32 != 0
}

#[cfg(target_os = "windows")]
fn windows_volume_root(path: &Path) -> Option<PathBuf> {
    use std::path::{Component, Prefix};

    let Component::Prefix(prefix) = path.components().next()? else {
        return None;
    };
    match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
            Some(PathBuf::from(format!("{}:\\", char::from(letter))))
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            let mut root = PathBuf::from(r"\\");
            root.push(server);
            root.push(share);
            root.as_mut_os_string().push("\\");
            Some(root)
        }
        _ => None,
    }
}

pub fn open_in_file_manager(path: &Path) -> Result<(), FsError> {
    if !path.exists() {
        return Err(FsError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("path does not exist: {}", path.display()),
        )));
    }

    #[cfg(target_os = "macos")]
    if path.is_dir() {
        Command::new("open").arg(path).spawn()?;
    } else {
        Command::new("open").arg("-R").arg(path).spawn()?;
    }
    #[cfg(target_os = "windows")]
    if path.is_dir() {
        Command::new("explorer.exe").arg(path).spawn()?;
    } else {
        Command::new("explorer.exe")
            .arg("/select,")
            .arg(path)
            .spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    Command::new("xdg-open")
        .arg(if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        })
        .spawn()?;

    Ok(())
}

pub fn sidecar_path(path: &Path) -> PathBuf {
    path.with_extension("xmp")
}

fn summary_for_path(path: &Path) -> Result<Option<AssetSummary>, FsError> {
    summary_for_path_with_sidecar(path, None)
}

fn summary_for_path_with_sidecar(
    path: &Path,
    known_sidecar: Option<bool>,
) -> Result<Option<AssetSummary>, FsError> {
    let Some(_) = AssetKind::from_path(path) else {
        return Ok(None);
    };
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    summary_from_metadata(
        path,
        &metadata,
        known_sidecar.unwrap_or_else(|| sidecar_path(path).is_file()),
    )
}

fn summary_from_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    has_sidecar: bool,
) -> Result<Option<AssetSummary>, FsError> {
    summary_from_attributes(
        path,
        metadata.len(),
        metadata.modified().map(epoch_ms).unwrap_or_default(),
        has_sidecar,
    )
}

fn summary_from_attributes(
    path: &Path,
    size_bytes: u64,
    modified_at_ms: u64,
    has_sidecar: bool,
) -> Result<Option<AssetSummary>, FsError> {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    let Some(kind) = AssetKind::from_extension(extension) else {
        return Ok(None);
    };
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(FsError::InvalidFileName)?
        .to_owned();

    Ok(Some(AssetSummary {
        id: stable_id(&path.to_string_lossy()),
        path: path.to_owned(),
        name,
        extension: extension.to_ascii_uppercase(),
        kind,
        size_bytes,
        modified_at_ms,
        has_sidecar,
        rating: None,
        color_label: None,
        pick_label: None,
    }))
}

fn compare_assets(left: &AssetSummary, right: &AssetSummary, sort: AssetSort) -> Ordering {
    let ordering = match sort {
        AssetSort::Name => left
            .name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase()),
        AssetSort::Modified => left.modified_at_ms.cmp(&right.modified_at_ms),
        AssetSort::Size => left.size_bytes.cmp(&right.size_bytes),
        AssetSort::Kind => format!("{:?}", left.kind).cmp(&format!("{:?}", right.kind)),
    };
    ordering.then_with(|| left.name.cmp(&right.name))
}

fn rename_asset(source: &Path, new_name: &str) -> Result<FileOperationResult, FsError> {
    let mut components = Path::new(new_name).components();
    if new_name.is_empty()
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(FsError::InvalidFileName);
    }
    let destination = source
        .parent()
        .ok_or(FsError::InvalidFileName)?
        .join(new_name);
    ensure_available(&destination)?;
    fs::rename(source, &destination)?;
    let mut affected_paths = vec![destination.clone()];
    move_sidecar(source, &destination, &mut affected_paths)?;
    Ok(FileOperationResult { affected_paths })
}

fn transfer_assets(
    sources: &[PathBuf],
    destination_dir: &Path,
    move_files: bool,
) -> Result<FileOperationResult, FsError> {
    fs::create_dir_all(destination_dir)?;
    let mut affected_paths = Vec::new();
    for source in sources {
        let file_name = source.file_name().ok_or(FsError::InvalidFileName)?;
        let destination = destination_dir.join(file_name);
        ensure_available(&destination)?;
        if move_files {
            fs::rename(source, &destination)?;
            move_sidecar(source, &destination, &mut affected_paths)?;
        } else {
            fs::copy(source, &destination)?;
            copy_sidecar(source, &destination, &mut affected_paths)?;
        }
        affected_paths.push(destination);
    }
    Ok(FileOperationResult { affected_paths })
}

fn trash_assets(paths: &[PathBuf]) -> Result<FileOperationResult, FsError> {
    let mut affected_paths = Vec::new();
    for path in paths {
        let sidecar = path.is_file().then(|| sidecar_path(path));
        trash::delete(path).map_err(|error| FsError::Trash(error.to_string()))?;
        affected_paths.push(path.clone());
        if let Some(sidecar) = sidecar.filter(|candidate| candidate.exists()) {
            trash::delete(&sidecar).map_err(|error| FsError::Trash(error.to_string()))?;
            affected_paths.push(sidecar);
        }
    }
    Ok(FileOperationResult { affected_paths })
}

fn delete_assets_permanently(paths: &[PathBuf]) -> Result<FileOperationResult, FsError> {
    if let Some(path) = paths
        .iter()
        .find(|path| deletion_mode_for_path(path) != FileDeletionMode::Permanent)
    {
        return Err(FsError::PermanentDeleteUnsupported(path.clone()));
    }

    let mut affected_paths = Vec::new();
    for path in paths {
        delete_path_permanently(path, &mut affected_paths)?;
    }
    Ok(FileOperationResult { affected_paths })
}

fn delete_path_permanently(path: &Path, affected_paths: &mut Vec<PathBuf>) -> Result<(), FsError> {
    let metadata = fs::symlink_metadata(path)?;
    let sidecar = metadata.is_file().then(|| sidecar_path(path));
    if metadata.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    affected_paths.push(path.to_owned());
    if let Some(sidecar) = sidecar.filter(|candidate| candidate.exists()) {
        fs::remove_file(&sidecar)?;
        affected_paths.push(sidecar);
    }
    Ok(())
}

fn move_sidecar(
    source: &Path,
    destination: &Path,
    affected: &mut Vec<PathBuf>,
) -> Result<(), FsError> {
    let source_sidecar = sidecar_path(source);
    if source_sidecar.exists() {
        let destination_sidecar = sidecar_path(destination);
        ensure_available(&destination_sidecar)?;
        fs::rename(source_sidecar, &destination_sidecar)?;
        affected.push(destination_sidecar);
    }
    Ok(())
}

fn copy_sidecar(
    source: &Path,
    destination: &Path,
    affected: &mut Vec<PathBuf>,
) -> Result<(), FsError> {
    let source_sidecar = sidecar_path(source);
    if source_sidecar.exists() {
        let destination_sidecar = sidecar_path(destination);
        ensure_available(&destination_sidecar)?;
        fs::copy(source_sidecar, &destination_sidecar)?;
        affected.push(destination_sidecar);
    }
    Ok(())
}

fn ensure_available(path: &Path) -> Result<(), FsError> {
    if path.exists() {
        Err(FsError::DestinationExists(path.to_owned()))
    } else {
        Ok(())
    }
}

fn stable_id(value: &impl Hash) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn epoch_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn discovers_supported_files_and_pairs_sidecars() {
        let directory = tempdir().unwrap();
        File::create(directory.path().join("one.CR3")).unwrap();
        File::create(directory.path().join("one.xmp")).unwrap();
        File::create(directory.path().join("two.jpg")).unwrap();
        File::create(directory.path().join("ignore.txt")).unwrap();

        let page = scan_directory(directory.path(), &AssetQuery::default(), 0).unwrap();
        assert_eq!(page.total, 2);
        assert!(page.items[0].has_sidecar);
        assert_eq!(page.items[0].kind, AssetKind::Raw);
    }

    #[test]
    fn discovers_hif_files_and_lists_only_the_current_directory_level() {
        let directory = tempdir().unwrap();
        File::create(directory.path().join("canon.HIF")).unwrap();
        fs::create_dir(directory.path().join("Portraits")).unwrap();
        fs::create_dir_all(directory.path().join("Trips").join("Coast")).unwrap();

        let page = scan_directory(directory.path(), &AssetQuery::default(), 0).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].kind, AssetKind::Heif);

        let children = list_directories(directory.path()).unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].name, "Portraits");
        assert!(children[0].has_children);
        assert!(children[1].has_children);

        let nested = list_directories(&children[1].path).unwrap();
        assert_eq!(nested.len(), 1);
        assert_eq!(nested[0].name, "Coast");
    }

    #[test]
    fn index_scan_returns_one_cheap_directory_batch() {
        let directory = tempdir().unwrap();
        File::create(directory.path().join("root.jpg")).unwrap();
        let child = directory.path().join("child");
        fs::create_dir(&child).unwrap();
        File::create(child.join("nested.jpg")).unwrap();

        let scan = scan_index_directory(directory.path()).unwrap();

        assert_eq!(scan.assets.len(), 1);
        assert_eq!(scan.assets[0].name, "root.jpg");
        assert_eq!(scan.directories.len(), 1);
        assert_eq!(scan.directories[0].name, "child");
    }

    #[test]
    fn scan_progress_is_batched_without_changing_complete_summaries() {
        let directory = tempdir().unwrap();
        for index in 0..513 {
            fs::write(
                directory.path().join(format!("image-{index:04}.jpg")),
                vec![b'x'; index % 17 + 1],
            )
            .unwrap();
        }
        fs::write(directory.path().join("image-0001.xmp"), "sidecar").unwrap();

        let mut progress = Vec::new();
        let mut reported = scan_assets_with_progress(directory.path(), |update| {
            progress.push(update);
        })
        .unwrap();
        let mut unreported = scan_index_assets(directory.path()).unwrap();
        reported.sort_by(|left, right| left.name.cmp(&right.name));
        unreported.sort_by(|left, right| left.name.cmp(&right.name));

        assert_eq!(reported, unreported);
        assert_eq!(reported.len(), 513);
        assert_eq!(reported[1].size_bytes, 2);
        assert!(reported[1].has_sidecar);
        assert_eq!(progress.first().unwrap().discovered_count, 0);
        assert!(!progress.first().unwrap().reading_attributes);
        assert!(
            progress
                .iter()
                .any(|update| update.reading_attributes && update.attributes_ms == 0)
        );
        let completed = progress.last().unwrap();
        assert!(completed.reading_attributes);
        assert_eq!(completed.discovered_count, reported.len());
        assert!(
            progress.len() <= 8,
            "progress was not batched: {}",
            progress.len()
        );
    }

    #[test]
    fn index_asset_scan_pairs_sidecars_from_one_directory_enumeration() {
        let directory = tempdir().unwrap();
        File::create(directory.path().join("paired.CR3")).unwrap();
        File::create(directory.path().join("paired.XMP")).unwrap();
        File::create(directory.path().join("CaseSensitive.CR3")).unwrap();
        File::create(directory.path().join("casesensitive.XMP")).unwrap();
        File::create(directory.path().join("plain.jpg")).unwrap();

        let assets = scan_index_assets(directory.path()).unwrap();

        assert_eq!(assets.len(), 3);
        assert!(
            assets
                .iter()
                .find(|asset| asset.name == "paired.CR3")
                .unwrap()
                .has_sidecar
        );
        assert!(
            !assets
                .iter()
                .find(|asset| asset.name == "plain.jpg")
                .unwrap()
                .has_sidecar
        );
        assert_eq!(
            assets
                .iter()
                .find(|asset| asset.name == "CaseSensitive.CR3")
                .unwrap()
                .has_sidecar,
            cfg!(windows)
        );
    }

    #[test]
    fn session_navigation_stays_within_the_opened_root() {
        let root = tempdir().unwrap();
        let child = root.path().join("child");
        fs::create_dir(&child).unwrap();
        File::create(child.join("photo.jpg")).unwrap();
        let outside = tempdir().unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(root.path()).unwrap();

        let page = catalog
            .list_assets(&session.id, Some(&child), &AssetQuery::default(), None)
            .unwrap();
        assert_eq!(page.total, 1);
        assert!(matches!(
            catalog.list_directories(&session.id, Some(outside.path())),
            Err(FsError::OutsideSessionRoot(_))
        ));
    }

    #[test]
    fn directory_cache_is_reused_until_refreshed() {
        let root = tempdir().unwrap();
        File::create(root.path().join("one.jpg")).unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(root.path()).unwrap();

        let first = catalog
            .list_assets(&session.id, None, &AssetQuery::default(), None)
            .unwrap();
        assert_eq!(first.total, 1);

        File::create(root.path().join("two.jpg")).unwrap();
        let cached = catalog
            .list_assets(&session.id, None, &AssetQuery::default(), None)
            .unwrap();
        assert_eq!(cached.total, 1);

        assert!(
            catalog
                .list_directories(&session.id, None)
                .unwrap()
                .is_empty()
        );
        fs::create_dir(root.path().join("new-folder")).unwrap();
        assert!(
            catalog
                .list_directories(&session.id, None)
                .unwrap()
                .is_empty()
        );

        catalog.refresh_directory(&session.id, None).unwrap();
        let refreshed = catalog
            .list_assets(&session.id, None, &AssetQuery::default(), None)
            .unwrap();
        assert_eq!(refreshed.total, 2);
        assert_eq!(
            catalog.list_directories(&session.id, None).unwrap().len(),
            1
        );
    }

    #[test]
    fn background_discovery_confirms_collapsed_branches_and_leaves() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("branch/leaf")).unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(root.path()).unwrap();
        let (snapshot, children) = catalog
            .prefetch_directory_children(&session.id, &session.root_path)
            .unwrap();
        assert!(!snapshot.root.expanded);
        assert_eq!(children.len(), 1);
        let (snapshot, leaves) = catalog
            .prefetch_directory_children(&session.id, &children[0])
            .unwrap();
        let branch = &snapshot.root.children.as_ref().unwrap()[0];
        assert!(!branch.expanded);
        assert!(branch.entry.has_children);
        let (snapshot, children) = catalog
            .prefetch_directory_children(&session.id, &leaves[0])
            .unwrap();
        assert!(children.is_empty());
        let leaf = find_tree_node(&snapshot.root, &leaves[0]).unwrap();
        assert!(!leaf.entry.has_children);
        assert_eq!(leaf.children, Some(Vec::new()));
        let (_, should_load) = catalog
            .set_directory_expanded(&session.id, &leaves[0], true)
            .unwrap();
        assert!(!should_load);
    }

    #[test]
    fn new_selection_completes_before_an_older_admitted_load() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("old")).unwrap();
        fs::create_dir_all(root.path().join("new/leaf")).unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(root.path()).unwrap();
        catalog
            .prefetch_directory_children(&session.id, &session.root_path)
            .unwrap();
        let old = session.root_path.join("old");
        let new = session.root_path.join("new");
        assert!(
            catalog
                .set_directory_expanded(&session.id, &old, true)
                .unwrap()
                .1
        );
        assert!(
            catalog
                .set_directory_expanded(&session.id, &new, true)
                .unwrap()
                .1
        );
        let snapshot = catalog.load_directory_children(&session.id, &new).unwrap();
        assert!(
            find_tree_node(&snapshot.root, &old)
                .unwrap()
                .children
                .is_none()
        );
        assert_eq!(
            find_tree_node(&snapshot.root, &new)
                .unwrap()
                .children
                .as_ref()
                .unwrap()
                .len(),
            1
        );
        catalog.load_directory_children(&session.id, &old).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn background_discovery_does_not_follow_symlink_cycles() {
        let root = tempdir().unwrap();
        std::os::unix::fs::symlink(root.path(), root.path().join("loop")).unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(root.path()).unwrap();
        let (_, children) = catalog
            .prefetch_directory_children(&session.id, &session.root_path)
            .unwrap();
        let (_, descendants) = catalog
            .prefetch_directory_children(&session.id, &children[0])
            .unwrap();
        assert!(descendants.is_empty());
    }

    #[test]
    fn rust_owns_lazy_directory_tree_expansion_and_leaf_detection() {
        let root = tempdir().unwrap();
        let branch = root.path().join("branch");
        let leaf = branch.join("leaf");
        fs::create_dir_all(&leaf).unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(root.path()).unwrap();
        let branch = session.root_path.join("branch");
        let leaf = branch.join("leaf");

        let initial = catalog.directory_tree(&session.id).unwrap();
        assert!(!initial.root.expanded);
        assert!(initial.root.children.is_none());
        assert_eq!(
            catalog
                .known_directory_tree_path(&session.id, &session.root_path)
                .unwrap(),
            session.root_path
        );
        assert!(matches!(
            catalog.known_directory_tree_path(&session.id, &branch),
            Err(FsError::InvalidFolder(_))
        ));

        let (root_loading, should_load) = catalog
            .set_directory_expanded(&session.id, &session.root_path, true)
            .unwrap();
        assert!(should_load);
        let (_, duplicate_load) = catalog
            .set_directory_expanded(&session.id, &session.root_path, true)
            .unwrap();
        assert!(!duplicate_load);
        assert!(root_loading.root.expanded);
        assert!(root_loading.root.children.is_none());
        let root_tree = catalog
            .load_directory_children(&session.id, &session.root_path)
            .unwrap();
        let root_children = root_tree.root.children.unwrap();
        assert_eq!(root_children.len(), 1);
        assert_eq!(root_children[0].entry.name, "branch");
        assert!(root_children[0].children.is_none());
        assert_eq!(
            catalog
                .known_directory_tree_path(&session.id, &branch)
                .unwrap(),
            branch
        );

        let (_, should_load) = catalog
            .set_directory_expanded(&session.id, &branch, true)
            .unwrap();
        assert!(should_load);
        let branch_tree = catalog
            .load_directory_children(&session.id, &branch)
            .unwrap();
        let branch_node = &branch_tree.root.children.as_ref().unwrap()[0];
        assert!(branch_node.expanded);
        assert_eq!(branch_node.children.as_ref().unwrap().len(), 1);

        let (_, should_load) = catalog
            .set_directory_expanded(&session.id, &leaf, true)
            .unwrap();
        assert!(should_load);
        let leaf_tree = catalog.load_directory_children(&session.id, &leaf).unwrap();
        let leaf_node = &leaf_tree.root.children.as_ref().unwrap()[0]
            .children
            .as_ref()
            .unwrap()[0];
        assert!(!leaf_node.expanded);
        assert!(!leaf_node.entry.has_children);
        assert!(leaf_node.children.as_ref().unwrap().is_empty());
        assert!(leaf_tree.revision > initial.revision);
    }

    #[test]
    fn refreshing_a_loaded_tree_level_preserves_nodes_and_removes_deleted_children() {
        let root = tempdir().unwrap();
        let child = root.path().join("child");
        fs::create_dir(&child).unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(root.path()).unwrap();
        let child = session.root_path.join("child");
        let (_, should_load) = catalog
            .set_directory_expanded(&session.id, &session.root_path, true)
            .unwrap();
        assert!(should_load);
        catalog
            .load_directory_children(&session.id, &session.root_path)
            .unwrap();

        fs::remove_dir(&child).unwrap();
        let refreshed = catalog.refresh_directory(&session.id, None).unwrap();

        assert!(!refreshed.root.entry.has_children);
        assert!(refreshed.root.children.as_ref().unwrap().is_empty());
    }

    fn discover_tree(catalog: &FsCatalog, session: &FolderSession) -> DirectoryTreeSnapshot {
        let mut pending = vec![session.root_path.clone()];
        while let Some(path) = pending.pop() {
            let (_, children) = catalog
                .prefetch_directory_children(&session.id, &path)
                .unwrap();
            pending.extend(children);
        }
        catalog.directory_tree(&session.id).unwrap()
    }

    #[test]
    fn refresh_from_a_child_rescans_loaded_siblings_and_empty_descendants() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("selected/keep")).unwrap();
        fs::create_dir_all(root.path().join("sibling/empty")).unwrap();
        fs::create_dir_all(root.path().join("sibling/deleted")).unwrap();
        fs::create_dir_all(root.path().join("sibling/old-name")).unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(root.path()).unwrap();
        discover_tree(&catalog, &session);
        let selected = session.root_path.join("selected");
        catalog
            .set_directory_expanded(&session.id, &session.root_path, true)
            .unwrap();
        catalog
            .set_directory_expanded(&session.id, &selected, true)
            .unwrap();
        let sibling = session.root_path.join("sibling");
        // Also populate the legacy directory-list cache: tree refresh must
        // read the filesystem, even when another consumer has cached this level.
        catalog
            .list_directories(&session.id, Some(&sibling))
            .unwrap();
        fs::remove_dir(sibling.join("deleted")).unwrap();
        fs::rename(sibling.join("old-name"), sibling.join("new-name")).unwrap();
        fs::create_dir_all(sibling.join("empty/new/deep")).unwrap();
        fs::create_dir(session.root_path.join("new-sibling")).unwrap();

        catalog
            .refresh_directory(&session.id, Some(&selected))
            .unwrap();
        let tree = discover_tree(&catalog, &session);
        assert!(tree.root.expanded);
        assert!(find_tree_node(&tree.root, &selected).unwrap().expanded);
        assert!(!find_tree_node(&tree.root, &sibling).unwrap().expanded);
        assert!(find_tree_node(&tree.root, &sibling.join("deleted")).is_none());
        assert!(find_tree_node(&tree.root, &sibling.join("old-name")).is_none());
        assert!(find_tree_node(&tree.root, &sibling.join("new-name")).is_some());
        assert!(find_tree_node(&tree.root, &sibling.join("empty/new/deep")).is_some());
        assert!(find_tree_node(&tree.root, &session.root_path.join("new-sibling")).is_some());
        assert!(
            find_tree_node(&tree.root, &sibling.join("empty"))
                .unwrap()
                .entry
                .has_children
        );

        // A completed generation continues to reuse its tree without IO.
        catalog
            .prefetch_directory_children_with(&session.id, &sibling, |_| {
                panic!("a completed level should not be scanned twice")
            })
            .unwrap();
    }

    #[test]
    fn refresh_rejects_a_late_background_tree_scan() {
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("old")).unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(root.path()).unwrap();
        let (tree, children) = catalog
            .prefetch_directory_children_with(&session.id, &session.root_path, |path| {
                let old = list_directories(path)?;
                fs::remove_dir(path.join("old"))?;
                fs::create_dir(path.join("new"))?;
                catalog.refresh_directory(&session.id, None)?;
                Ok(old)
            })
            .unwrap();
        assert!(children.is_empty());
        assert!(find_tree_node(&tree.root, &session.root_path.join("old")).is_none());
        assert!(find_tree_node(&tree.root, &session.root_path.join("new")).is_some());
        let final_tree = discover_tree(&catalog, &session);
        assert!(find_tree_node(&final_tree.root, &session.root_path.join("old")).is_none());
    }

    #[test]
    fn collapsing_a_queued_node_revokes_its_filesystem_load() {
        let root = tempdir().unwrap();
        let child = root.path().join("child");
        fs::create_dir(&child).unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(root.path()).unwrap();
        let child = session.root_path.join("child");
        let (_, should_load) = catalog
            .set_directory_expanded(&session.id, &session.root_path, true)
            .unwrap();
        assert!(should_load);
        catalog
            .load_directory_children(&session.id, &session.root_path)
            .unwrap();

        let (_, should_load) = catalog
            .set_directory_expanded(&session.id, &child, true)
            .unwrap();
        assert!(should_load);
        catalog
            .set_directory_expanded(&session.id, &child, false)
            .unwrap();
        fs::remove_dir(&child).unwrap();

        let tree = catalog
            .load_directory_children(&session.id, &child)
            .unwrap();
        let child = &tree.root.children.as_ref().unwrap()[0];
        assert!(!child.expanded);
        assert!(child.children.is_none());
    }

    #[test]
    fn collapsing_the_whole_tree_keeps_loaded_children_but_marks_every_level_closed() {
        let root = tempdir().unwrap();
        let child = root.path().join("child");
        fs::create_dir(&child).unwrap();
        let catalog = FsCatalog::default();
        let session = catalog.open_folder(root.path()).unwrap();
        let child = session.root_path.join("child");
        catalog
            .set_directory_expanded(&session.id, &session.root_path, true)
            .unwrap();
        catalog
            .load_directory_children(&session.id, &session.root_path)
            .unwrap();
        catalog
            .set_directory_expanded(&session.id, &child, true)
            .unwrap();

        let tree = catalog.collapse_directory_tree(&session.id).unwrap();

        assert!(!tree.root.expanded);
        let child_node = &tree.root.children.as_ref().unwrap()[0];
        assert!(!child_node.expanded);
    }

    #[test]
    fn paginates_and_filters_without_returning_sidecars() {
        let directory = tempdir().unwrap();
        for name in ["a.jpg", "b.jpg", "c.heic", "d.nef"] {
            File::create(directory.path().join(name)).unwrap();
        }
        let query = AssetQuery {
            kind: Some(AssetKind::Jpeg),
            page_size: Some(1),
            ..AssetQuery::default()
        };

        let first = scan_directory(directory.path(), &query, 0).unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.next_cursor, Some(1));
        assert_eq!(first.items.len(), 1);
    }

    #[test]
    fn name_sort_keeps_case_insensitive_order_and_tie_breaks_across_pages() {
        let directory = tempdir().unwrap();
        for name in ["one.jpg", "two.jpg", "three.jpg"] {
            File::create(directory.path().join(name)).unwrap();
        }
        let mut assets = scan_assets(directory.path()).unwrap();
        for (asset, name) in assets.iter_mut().zip(["a.jpg", "A.jpg", "b.jpg"]) {
            asset.name = name.into();
        }
        let query = AssetQuery {
            page_size: Some(2),
            ..AssetQuery::default()
        };

        let first = page_assets(&assets, &query, 0);
        let second = page_assets(&assets, &query, 2);
        assert_eq!(
            first
                .items
                .iter()
                .chain(&second.items)
                .map(|asset| asset.name.as_str())
                .collect::<Vec<_>>(),
            ["A.jpg", "a.jpg", "b.jpg"]
        );

        let descending = page_assets(
            &assets,
            &AssetQuery {
                direction: SortDirection::Descending,
                ..query
            },
            0,
        );
        assert_eq!(
            descending
                .items
                .iter()
                .map(|asset| asset.name.as_str())
                .collect::<Vec<_>>(),
            ["b.jpg", "a.jpg"]
        );
    }

    #[test]
    fn filters_enriched_summaries_by_minimum_rating_and_colors_with_or_logic() {
        let directory = tempdir().unwrap();
        for name in ["a.jpg", "b.jpg", "c.jpg"] {
            File::create(directory.path().join(name)).unwrap();
        }
        let mut assets = scan_assets(directory.path()).unwrap();
        let a = assets
            .iter_mut()
            .find(|asset| asset.name == "a.jpg")
            .unwrap();
        a.rating = Some(5);
        a.color_label = Some("Blue".into());
        let b = assets
            .iter_mut()
            .find(|asset| asset.name == "b.jpg")
            .unwrap();
        b.rating = Some(4);
        b.color_label = Some("Red".into());
        let query = AssetQuery {
            minimum_rating: Some(4),
            color_labels: vec!["blue".into(), "red".into()],
            ..AssetQuery::default()
        };

        let page = page_assets(&assets, &query, 0);
        assert_eq!(page.total, 2);
        assert_eq!(page.items[0].name, "a.jpg");
        assert_eq!(page.items[1].name, "b.jpg");
    }

    #[test]
    fn renames_raw_and_its_sidecar_together() {
        let directory = tempdir().unwrap();
        let raw = directory.path().join("before.nef");
        File::create(&raw).unwrap();
        File::create(sidecar_path(&raw)).unwrap();

        let result = execute_file_operation(&FileOperation::Rename {
            source: raw,
            new_name: "after.nef".into(),
        })
        .unwrap();

        assert_eq!(result.affected_paths.len(), 2);
        assert!(directory.path().join("after.nef").exists());
        assert!(directory.path().join("after.xmp").exists());
    }

    #[test]
    fn renames_hif_and_its_sidecar_together() {
        let directory = tempdir().unwrap();
        let hif = directory.path().join("before.HIF");
        File::create(&hif).unwrap();
        File::create(sidecar_path(&hif)).unwrap();

        execute_file_operation(&FileOperation::Rename {
            source: hif,
            new_name: "after.HIF".into(),
        })
        .unwrap();

        assert!(directory.path().join("after.HIF").exists());
        assert!(directory.path().join("after.xmp").exists());
        assert!(!directory.path().join("before.xmp").exists());
    }

    #[test]
    fn permanent_delete_removes_a_file_and_its_sidecar() {
        let directory = tempdir().unwrap();
        let raw = directory.path().join("delete.nef");
        let sidecar = sidecar_path(&raw);
        File::create(&raw).unwrap();
        File::create(&sidecar).unwrap();
        let mut affected = Vec::new();

        delete_path_permanently(&raw, &mut affected).unwrap();

        assert_eq!(affected, [raw.clone(), sidecar.clone()]);
        assert!(!raw.exists());
        assert!(!sidecar.exists());
    }

    #[test]
    fn permanent_delete_removes_a_directory_tree() {
        let directory = tempdir().unwrap();
        let child = directory.path().join("delete-me");
        fs::create_dir(&child).unwrap();
        File::create(child.join("photo.jpg")).unwrap();
        let mut affected = Vec::new();

        delete_path_permanently(&child, &mut affected).unwrap();

        assert_eq!(affected, std::slice::from_ref(&child));
        assert!(!child.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn local_macos_volume_uses_system_trash() {
        let directory = tempdir().unwrap();

        assert_eq!(
            deletion_mode_for_path(directory.path()),
            FileDeletionMode::Trash
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unknown_macos_volume_uses_permanent_deletion() {
        assert_eq!(
            deletion_mode_for_path(Path::new("/path/that/does/not/exist")),
            FileDeletionMode::Permanent
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn extracts_unc_and_mapped_drive_roots_for_drive_type_detection() {
        assert_eq!(
            windows_volume_root(Path::new(r"\\server\share\photos\2026")),
            Some(PathBuf::from(r"\\server\share\")),
        );
        assert_eq!(
            windows_volume_root(Path::new(r"Z:\photos\2026")),
            Some(PathBuf::from(r"Z:\")),
        );
    }
}
