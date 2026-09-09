use oxy_domain::{CacheSettings, CacheSettingsUpdate};
use oxy_media::MediaCache;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        RwLock,
        atomic::{AtomicU8, Ordering},
    },
};

pub const DEFAULT_MAX_SIZE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub const MIN_MAX_SIZE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_MAX_SIZE_BYTES: u64 = 500 * 1024 * 1024 * 1024;
const CUSTOM_CACHE_FOLDER: &str = "OxyViewer Cache";
const PRUNE_IDLE: u8 = 0;
const PRUNE_RUNNING: u8 = 1;
const PRUNE_PENDING: u8 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedCacheConfig {
    #[serde(default)]
    custom_parent: Option<PathBuf>,
    #[serde(default = "default_max_size_bytes")]
    max_size_bytes: u64,
}

fn default_max_size_bytes() -> u64 {
    DEFAULT_MAX_SIZE_BYTES
}

pub struct CacheManager {
    default_preview_dir: PathBuf,
    config_path: PathBuf,
    config: RwLock<PersistedCacheConfig>,
    prune_state: AtomicU8,
}

impl CacheManager {
    pub fn load(default_preview_dir: PathBuf, config_path: PathBuf) -> Result<Self, String> {
        let mut config = fs::read(&config_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PersistedCacheConfig>(&bytes).ok())
            .filter(|config| valid_limit(config.max_size_bytes))
            .unwrap_or(PersistedCacheConfig {
                custom_parent: None,
                max_size_bytes: DEFAULT_MAX_SIZE_BYTES,
            });
        // A removable or disconnected custom volume must never prevent the
        // photo browser from opening. Fall back for this run; the user can
        // select the location again when it becomes available.
        if config
            .custom_parent
            .as_ref()
            .is_some_and(|parent| !parent.is_dir())
        {
            config.custom_parent = None;
        }
        let preview_dir =
            resolve_preview_dir(&default_preview_dir, config.custom_parent.as_deref());
        fs::create_dir_all(&preview_dir).map_err(|error| error.to_string())?;
        remove_legacy_cache_files(&preview_dir)?;
        if preview_dir != default_preview_dir {
            remove_legacy_cache_files(&default_preview_dir)?;
        }
        oxy_media::DiskMediaCache::new(&preview_dir, 256).map_err(|error| error.to_string())?;
        let manager = Self {
            default_preview_dir,
            config_path,
            config: RwLock::new(config),
            prune_state: AtomicU8::new(PRUNE_IDLE),
        };
        Ok(manager)
    }

    pub fn preview_dir(&self) -> PathBuf {
        let config = self
            .config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        resolve_preview_dir(&self.default_preview_dir, config.custom_parent.as_deref())
    }

    pub fn max_size_bytes(&self) -> u64 {
        self.config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .max_size_bytes
    }

    pub fn settings(&self) -> Result<CacheSettings, String> {
        let config = self
            .config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let location =
            resolve_preview_dir(&self.default_preview_dir, config.custom_parent.as_deref());
        let usage = oxy_media::DiskMediaCache::new(&location, 256)
            .and_then(|cache| cache.usage())
            .map_err(|error| error.to_string())?;
        Ok(CacheSettings {
            location,
            default_location: self.default_preview_dir.clone(),
            custom_parent: config.custom_parent.clone(),
            is_custom_location: config.custom_parent.is_some(),
            max_size_bytes: config.max_size_bytes,
            used_size_bytes: usage.size_bytes,
        })
    }

    pub fn update(&self, update: CacheSettingsUpdate) -> Result<CacheSettings, String> {
        if !valid_limit(update.max_size_bytes) {
            return Err("cache size must be between 1 GB and 500 GB".into());
        }
        let custom_parent = update
            .custom_parent
            .map(|parent| {
                if !parent.is_dir() {
                    return Err("selected cache parent is not a directory".to_owned());
                }
                parent.canonicalize().map_err(|error| error.to_string())
            })
            .transpose()?;
        let next = PersistedCacheConfig {
            custom_parent,
            max_size_bytes: update.max_size_bytes,
        };
        let preview_dir =
            resolve_preview_dir(&self.default_preview_dir, next.custom_parent.as_deref());
        fs::create_dir_all(&preview_dir).map_err(|error| error.to_string())?;
        remove_legacy_cache_files(&preview_dir)?;
        self.persist(&next)?;
        *self
            .config
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
        oxy_media::DiskMediaCache::new(&preview_dir, 256)
            .map_err(|error| error.to_string())?
            .prune(update.max_size_bytes)
            .map_err(|error| error.to_string())?;
        self.settings()
    }

    pub fn clear(&self) -> Result<CacheSettings, String> {
        let preview_dir = self.preview_dir();
        oxy_media::DiskMediaCache::new(&preview_dir, 256)
            .and_then(|cache| cache.clear())
            .map_err(|error| error.to_string())?;
        self.settings()
    }

    pub fn try_start_prune(&self) -> bool {
        loop {
            match self.prune_state.load(Ordering::Acquire) {
                PRUNE_IDLE => {
                    if self
                        .prune_state
                        .compare_exchange(
                            PRUNE_IDLE,
                            PRUNE_RUNNING,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return true;
                    }
                }
                PRUNE_RUNNING => {
                    if self
                        .prune_state
                        .compare_exchange(
                            PRUNE_RUNNING,
                            PRUNE_PENDING,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return false;
                    }
                }
                PRUNE_PENDING => return false,
                _ => unreachable!("invalid prune state"),
            }
        }
    }

    /// Completes one prune pass. Returns true while this worker owns a latched
    /// follow-up pass requested by a publication that overlapped the first.
    pub fn finish_prune(&self) -> bool {
        loop {
            match self.prune_state.load(Ordering::Acquire) {
                PRUNE_PENDING => {
                    if self
                        .prune_state
                        .compare_exchange(
                            PRUNE_PENDING,
                            PRUNE_RUNNING,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return true;
                    }
                }
                PRUNE_RUNNING => {
                    if self
                        .prune_state
                        .compare_exchange(
                            PRUNE_RUNNING,
                            PRUNE_IDLE,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return false;
                    }
                }
                PRUNE_IDLE => return false,
                _ => unreachable!("invalid prune state"),
            }
        }
    }

    pub fn prune_after_write(&self, protected_path: &Path) -> Result<(), String> {
        let cache_dir = self.preview_dir();
        // An in-flight decode may have completed in the previous location.
        // Leave that old artifact alone; the selected location is authoritative
        // for all subsequent requests.
        let cache =
            oxy_media::DiskMediaCache::new(&cache_dir, 256).map_err(|error| error.to_string())?;
        if !protected_path.starts_with(cache.root()) {
            return Ok(());
        }
        cache
            .prune_with_protected(self.max_size_bytes(), Some(protected_path))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn persist(&self, config: &PersistedCacheConfig) -> Result<(), String> {
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?;
        fs::write(&self.config_path, bytes).map_err(|error| error.to_string())
    }
}

fn resolve_preview_dir(default: &Path, custom_parent: Option<&Path>) -> PathBuf {
    custom_parent.map_or_else(
        || default.to_owned(),
        |parent| parent.join(CUSTOM_CACHE_FOLDER).join("previews"),
    )
}

fn valid_limit(value: u64) -> bool {
    (MIN_MAX_SIZE_BYTES..=MAX_MAX_SIZE_BYTES).contains(&value)
}

/// Removes the pre-v2 flat cache format without traversing directories or
/// following links. The preview directory is application-owned even when its
/// parent was selected by the user.
fn remove_legacy_cache_files(cache_dir: &Path) -> Result<(), String> {
    let entries = match fs::read_dir(cache_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if !file_type.is_file() {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    #[test]
    fn custom_cache_is_always_scoped_to_an_app_owned_child() {
        let parent = tempfile::tempdir().unwrap();
        let default = parent.path().join("default/previews");
        let manager = CacheManager::load(default, parent.path().join("settings.json")).unwrap();

        let settings = manager
            .update(CacheSettingsUpdate {
                custom_parent: Some(parent.path().to_owned()),
                max_size_bytes: DEFAULT_MAX_SIZE_BYTES,
            })
            .unwrap();

        assert_eq!(
            settings.location,
            parent
                .path()
                .canonicalize()
                .unwrap()
                .join(CUSTOM_CACHE_FOLDER)
                .join("previews")
        );
        assert!(settings.is_custom_location);
    }

    #[test]
    fn startup_removes_flat_cache_files_and_preserves_owned_directories() {
        let parent = tempfile::tempdir().unwrap();
        let preview_dir = parent.path().join("previews");
        std::fs::create_dir_all(&preview_dir).unwrap();
        let legacy = preview_dir.join("legacy-preview.jpg");
        std::fs::write(&legacy, b"legacy").unwrap();
        let nested = preview_dir.join("unrelated");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("keep.txt"), b"keep").unwrap();

        CacheManager::load(preview_dir.clone(), parent.path().join("settings.json")).unwrap();

        assert!(!legacy.exists());
        assert_eq!(std::fs::read(nested.join("keep.txt")).unwrap(), b"keep");
        assert!(preview_dir.join("media-cache-v2").is_dir());
    }

    #[test]
    fn startup_cleans_configured_and_default_cache_locations() {
        let parent = tempfile::tempdir().unwrap();
        let default = parent.path().join("default/previews");
        let custom_parent = parent.path().join("custom");
        let custom = custom_parent.join(CUSTOM_CACHE_FOLDER).join("previews");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::create_dir_all(&custom).unwrap();
        let default_legacy = default.join("old-default.jpg");
        let custom_legacy = custom.join("old-custom.jpg");
        std::fs::write(&default_legacy, b"legacy").unwrap();
        std::fs::write(&custom_legacy, b"legacy").unwrap();
        let config_path = parent.path().join("settings.json");
        std::fs::write(
            &config_path,
            serde_json::to_vec(&PersistedCacheConfig {
                custom_parent: Some(custom_parent.canonicalize().unwrap()),
                max_size_bytes: DEFAULT_MAX_SIZE_BYTES,
            })
            .unwrap(),
        )
        .unwrap();

        let manager = CacheManager::load(default, config_path).unwrap();

        assert_eq!(manager.preview_dir(), custom.canonicalize().unwrap());
        assert!(!default_legacy.exists());
        assert!(!custom_legacy.exists());
    }

    #[test]
    fn completion_prune_latches_an_overlapping_follow_up() {
        let parent = tempfile::tempdir().unwrap();
        let manager = CacheManager::load(
            parent.path().join("default/previews"),
            parent.path().join("settings.json"),
        )
        .unwrap();

        assert!(manager.try_start_prune());
        assert!(!manager.try_start_prune());
        assert!(manager.finish_prune());
        assert!(!manager.finish_prune());
        assert!(manager.try_start_prune());
        assert!(!manager.finish_prune());
    }

    #[test]
    fn completion_prune_preserves_a_barrier_overlapped_trigger() {
        let parent = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            CacheManager::load(
                parent.path().join("default/previews"),
                parent.path().join("settings.json"),
            )
            .unwrap(),
        );
        assert!(manager.try_start_prune());
        let barrier = Arc::new(Barrier::new(2));
        let requester = {
            let manager = Arc::clone(&manager);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                assert!(!manager.try_start_prune());
            })
        };
        barrier.wait();
        requester.join().unwrap();
        assert!(manager.finish_prune());
        assert!(!manager.finish_prune());
    }

    #[test]
    fn invalid_capacity_is_rejected() {
        let parent = tempfile::tempdir().unwrap();
        let manager = CacheManager::load(
            parent.path().join("default/previews"),
            parent.path().join("settings.json"),
        )
        .unwrap();

        assert!(
            manager
                .update(CacheSettingsUpdate {
                    custom_parent: None,
                    max_size_bytes: MIN_MAX_SIZE_BYTES - 1,
                })
                .is_err()
        );
    }
}
