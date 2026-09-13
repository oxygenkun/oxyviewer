use oxy_domain::{ExternalAppSettings, ExternalApplication};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::PathBuf,
    sync::{Mutex, RwLock},
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedSettings {
    version: u32,
    #[serde(flatten)]
    settings: ExternalAppSettings,
}

pub(crate) struct ExternalAppManager {
    path: PathBuf,
    settings: RwLock<Result<ExternalAppSettings, String>>,
    // Serialize writes without holding the read lock during file I/O.
    save_gate: Mutex<()>,
}

impl ExternalAppManager {
    pub(crate) fn load(path: PathBuf) -> Self {
        let settings = (|| {
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(ExternalAppSettings::default());
                }
                Err(error) => return Err(error.to_string()),
            };
            let persisted: PersistedSettings =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            if persisted.version != 1 {
                return Err("Unsupported external application settings version".into());
            }
            normalize(persisted.settings)
        })();
        Self {
            path,
            settings: RwLock::new(settings),
            save_gate: Mutex::new(()),
        }
    }

    pub(crate) fn get(&self) -> Result<ExternalAppSettings, String> {
        self.settings
            .read()
            .map_err(|error| error.to_string())?
            .clone()
    }

    pub(crate) fn application(&self, id: &str) -> Result<ExternalApplication, String> {
        self.get()?
            .apps
            .into_iter()
            .find(|app| app.id == id)
            .ok_or_else(|| "Application is no longer configured".into())
    }

    pub(crate) fn update(
        &self,
        settings: ExternalAppSettings,
    ) -> Result<ExternalAppSettings, String> {
        let _save = self.save_gate.lock().map_err(|error| error.to_string())?;
        let settings = normalize(settings)?;
        let previous = self.get().unwrap_or_default();
        // A removed executable must not prevent reordering/removing other entries.
        for app in &settings.apps {
            if !previous
                .apps
                .iter()
                .any(|old| old.id == app.id && old.executable_path == app.executable_path)
            {
                oxy_fs::external_apps::validate_application(&app.executable_path)
                    .map_err(|error| error.to_string())?;
            }
        }
        let parent = self.path.parent().ok_or("Missing settings directory")?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let mut file =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
        let persisted = PersistedSettings {
            version: 1,
            settings: settings.clone(),
        };
        serde_json::to_writer_pretty(&mut file, &persisted).map_err(|error| error.to_string())?;
        file.flush()
            .and_then(|()| file.as_file().sync_all())
            .map_err(|error| error.to_string())?;
        file.persist(&self.path)
            .map_err(|error| error.to_string())?;
        *self.settings.write().map_err(|error| error.to_string())? = Ok(settings.clone());
        Ok(settings)
    }
}

fn normalize(mut settings: ExternalAppSettings) -> Result<ExternalAppSettings, String> {
    let mut ids = HashSet::new();
    for app in &mut settings.apps {
        app.name = app.name.trim().to_owned();
        if app.id.is_empty()
            || !ids.insert(app.id.clone())
            || app.name.is_empty()
            || !app.executable_path.is_absolute()
        {
            return Err("Applications need unique IDs, names and absolute program paths".into());
        }
    }
    if !settings
        .default_app_id
        .as_ref()
        .is_some_and(|id| ids.contains(id))
    {
        settings.default_app_id = settings.apps.first().map(|app| app.id.clone());
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_order_and_default_and_allows_removing_missing_application() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("编辑器 A.exe");
        fs::write(&executable, b"fixture").unwrap();
        let path = dir.path().join("external-apps.json");
        let manager = ExternalAppManager::load(path.clone());
        let apps = ["b", "a"]
            .map(|id| ExternalApplication {
                id: id.into(),
                name: id.into(),
                executable_path: executable.clone(),
            })
            .to_vec();
        let settings = manager
            .update(ExternalAppSettings {
                apps,
                default_app_id: Some("a".into()),
            })
            .unwrap();
        assert_eq!(ExternalAppManager::load(path).get().unwrap(), settings);
        fs::remove_file(executable).unwrap();
        let updated = manager
            .update(ExternalAppSettings {
                apps: settings.apps[..1].to_vec(),
                default_app_id: Some("a".into()),
            })
            .unwrap();
        assert_eq!(updated.default_app_id.as_deref(), Some("b"));
        assert_eq!(
            manager
                .update(ExternalAppSettings::default())
                .unwrap()
                .default_app_id,
            None
        );
    }

    #[test]
    fn rejected_update_does_not_replace_saved_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("external-apps.json");
        let manager = ExternalAppManager::load(path.clone());
        manager.update(ExternalAppSettings::default()).unwrap();
        let before = fs::read(&path).unwrap();
        let app = ExternalApplication {
            id: "a".into(),
            name: "Missing".into(),
            executable_path: dir.path().join("missing.exe"),
        };
        assert!(
            manager
                .update(ExternalAppSettings {
                    apps: vec![app],
                    default_app_id: None
                })
                .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), before);
        assert_eq!(manager.get().unwrap(), ExternalAppSettings::default());
    }
}
