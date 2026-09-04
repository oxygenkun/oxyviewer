use oxy_domain::{CacheSettings, CacheSettingsUpdate};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, FileTimes},
    path::{Path, PathBuf},
    sync::{
        RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

pub const DEFAULT_MAX_SIZE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub const MIN_MAX_SIZE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_MAX_SIZE_BYTES: u64 = 500 * 1024 * 1024 * 1024;
const CUSTOM_CACHE_FOLDER: &str = "OxyViewer Cache";

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
    prune_running: AtomicBool,
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
        let manager = Self {
            default_preview_dir,
            config_path,
            config: RwLock::new(config),
            prune_running: AtomicBool::new(false),
        };
        fs::create_dir_all(manager.preview_dir()).map_err(|error| error.to_string())?;
        Ok(manager)
    }

    pub fn preview_dir(&self) -> PathBuf {
        let config = self
            .config
            .read()
            .unwrap_or_else(|error| error.into_inner());
        resolve_preview_dir(&self.default_preview_dir, config.custom_parent.as_deref())
    }

    pub fn max_size_bytes(&self) -> u64 {
        self.config
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .max_size_bytes
    }

    pub fn settings(&self) -> Result<CacheSettings, String> {
        let config = self
            .config
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let location =
            resolve_preview_dir(&self.default_preview_dir, config.custom_parent.as_deref());
        let usage = oxy_media::preview_cache_usage(&location).map_err(|error| error.to_string())?;
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
        self.persist(&next)?;
        *self
            .config
            .write()
            .unwrap_or_else(|error| error.into_inner()) = next;
        oxy_media::prune_preview_cache(&preview_dir, update.max_size_bytes, None)
            .map_err(|error| error.to_string())?;
        self.settings()
    }

    pub fn clear(&self) -> Result<CacheSettings, String> {
        let preview_dir = self.preview_dir();
        oxy_media::clear_preview_cache(&preview_dir).map_err(|error| error.to_string())?;
        self.settings()
    }

    pub fn mark_used(&self, path: &Path) {
        if path.parent() != Some(self.preview_dir().as_path()) {
            return;
        }
        if let Ok(file) = File::options().write(true).open(path) {
            let _ = file.set_times(FileTimes::new().set_modified(SystemTime::now()));
        }
    }

    pub fn try_start_prune(&self) -> bool {
        self.prune_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn finish_prune(&self) {
        self.prune_running.store(false, Ordering::Release);
    }

    pub fn prune_after_write(&self, protected_path: &Path) -> Result<(), String> {
        let cache_dir = self.preview_dir();
        // An in-flight decode may have completed in the previous location.
        // Leave that old artifact alone; the selected location is authoritative
        // for all subsequent requests.
        if protected_path.parent() != Some(cache_dir.as_path()) {
            return Ok(());
        }
        oxy_media::prune_preview_cache(&cache_dir, self.max_size_bytes(), Some(protected_path))
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
    custom_parent
        .map(|parent| parent.join(CUSTOM_CACHE_FOLDER).join("previews"))
        .unwrap_or_else(|| default.to_owned())
}

fn valid_limit(value: u64) -> bool {
    (MIN_MAX_SIZE_BYTES..=MAX_MAX_SIZE_BYTES).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

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
