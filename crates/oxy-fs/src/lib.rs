use oxy_domain::{
    AssetKind, AssetQuery, AssetSort, AssetSummary, DirectorySummary, FileOperation,
    FileOperationResult, FolderSession, Page, SortDirection,
};
use parking_lot::RwLock;
use std::{
    cmp::Ordering,
    collections::{HashMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

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
}

#[derive(Default)]
pub struct FsCatalog {
    sessions: RwLock<HashMap<String, PathBuf>>,
    asset_cache: RwLock<HashMap<PathBuf, Arc<Vec<AssetSummary>>>>,
    directory_cache: RwLock<HashMap<PathBuf, Arc<Vec<DirectorySummary>>>>,
}

impl FsCatalog {
    pub fn open_folder(&self, path: impl AsRef<Path>) -> Result<FolderSession, FsError> {
        let root_path = path.as_ref().canonicalize()?;
        if !root_path.is_dir() {
            return Err(FsError::InvalidFolder(root_path));
        }

        let opened_at_ms = epoch_ms(SystemTime::now());
        let id = stable_id(&(root_path.clone(), opened_at_ms));
        self.sessions.write().insert(id.clone(), root_path.clone());

        let display_name = root_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| root_path.to_string_lossy().into_owned());

        Ok(FolderSession {
            id,
            display_name,
            root_path,
            opened_at_ms,
        })
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

    pub fn refresh_directory(
        &self,
        session_id: &str,
        directory: Option<&Path>,
    ) -> Result<(), FsError> {
        let directory = self.resolve_session_directory(session_id, directory)?;
        self.asset_cache.write().remove(&directory);
        self.directory_cache.write().remove(&directory);
        Ok(())
    }

    pub fn invalidate_directory(&self, directory: &Path) {
        self.asset_cache.write().remove(directory);
        self.directory_cache.write().remove(directory);
    }

    pub fn get_asset(&self, path: impl AsRef<Path>) -> Result<AssetSummary, FsError> {
        summary_for_path(path.as_ref())?.ok_or(FsError::InvalidFileName)
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
}

pub fn list_directories(root: &Path) -> Result<Vec<DirectorySummary>, FsError> {
    if !root.is_dir() {
        return Err(FsError::InvalidFolder(root.to_owned()));
    }
    let mut directories = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
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
                // Do not probe this directory before returning the current level.
                // The directory tree loads its children on expansion and replaces
                // this optimistic value with the actual result.
                has_children: true,
            });
            continue;
        }
    }
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
    if !root.is_dir() {
        return Err(FsError::InvalidFolder(root.to_owned()));
    }
    let mut assets = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if let Some(summary) = summary_for_path(&entry.path())? {
            assets.push(summary);
        }
    }
    Ok(assets)
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
        if query.color_label.as_ref().is_some_and(|label| {
            summary
                .color_label
                .as_deref()
                .is_none_or(|value| !value.eq_ignore_ascii_case(label))
        }) {
            continue;
        }
        items.push(summary.clone());
    }

    items.sort_unstable_by(|left, right| compare_assets(left, right, query.sort));
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
        items.drain(offset..end).collect()
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
    }
}

pub fn sidecar_path(path: &Path) -> PathBuf {
    path.with_extension("xmp")
}

fn summary_for_path(path: &Path) -> Result<Option<AssetSummary>, FsError> {
    if !path.is_file() {
        return Ok(None);
    }
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    let Some(kind) = kind_for_extension(extension) else {
        return Ok(None);
    };
    let metadata = fs::metadata(path)?;
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
        size_bytes: metadata.len(),
        modified_at_ms: metadata.modified().map(epoch_ms).unwrap_or_default(),
        has_sidecar: sidecar_path(path).is_file(),
        rating: None,
        color_label: None,
    }))
}

fn kind_for_extension(extension: &str) -> Option<AssetKind> {
    match extension.to_ascii_lowercase().as_str() {
        "arw" | "cr2" | "cr3" | "nef" | "dng" | "raf" | "rw2" | "orf" => Some(AssetKind::Raw),
        "jpg" | "jpeg" => Some(AssetKind::Jpeg),
        "heif" | "heic" | "hif" => Some(AssetKind::Heif),
        "png" => Some(AssetKind::Png),
        "tif" | "tiff" => Some(AssetKind::Tiff),
        "webp" => Some(AssetKind::Webp),
        _ => None,
    }
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
        let sidecar = sidecar_path(path);
        trash::delete(path).map_err(|error| FsError::Trash(error.to_string()))?;
        affected_paths.push(path.clone());
        if sidecar.exists() {
            trash::delete(&sidecar).map_err(|error| FsError::Trash(error.to_string()))?;
            affected_paths.push(sidecar);
        }
    }
    Ok(FileOperationResult { affected_paths })
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
    fn filters_enriched_summaries_by_minimum_rating_and_color() {
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
            color_label: Some("blue".into()),
            ..AssetQuery::default()
        };

        let page = page_assets(&assets, &query, 0);
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].name, "a.jpg");
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
}
