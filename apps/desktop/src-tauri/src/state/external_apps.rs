//! The user's configured external applications.
//!
//! A configured editor is user data, not cache: losing it to a cache clear or a
//! truncated write means configuring it again. It lives in
//! `app_data_dir/external-apps.json` through the shared [`DocumentStore`], which
//! owns the atomic replacement and the refusal to overwrite a file this build
//! does not understand.

use oxy_domain::{ExternalAppSettings, ExternalApplication};
use oxy_userdata::{DocumentStore, StoreOrigin, UserDocument};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::PathBuf};

/// On-disk schema for the application list.
const SETTINGS_VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedSettings {
    #[serde(default = "settings_version")]
    version: u32,
    #[serde(flatten, default)]
    settings: ExternalAppSettings,
}

fn settings_version() -> u32 {
    SETTINGS_VERSION
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            settings: ExternalAppSettings::default(),
        }
    }
}

impl UserDocument for PersistedSettings {
    const KIND: &'static str = "external application settings";
    const CURRENT_VERSION: u32 = SETTINGS_VERSION;

    fn version(&self) -> u32 {
        self.version
    }

    fn stamp(&mut self) {
        self.version = SETTINGS_VERSION;
    }
}

pub(crate) struct ExternalAppManager {
    store: DocumentStore<PersistedSettings>,
}

impl ExternalAppManager {
    pub(crate) fn load(path: PathBuf) -> Self {
        let (mut store, origin) = DocumentStore::<PersistedSettings>::load_recoverable(path);
        if let StoreOrigin::Quarantined { path, reason } = &origin {
            eprintln!(
                "external application settings were unreadable ({reason}); kept the original as {}",
                path.display()
            );
        }
        // A file this application wrote always normalizes. If it does not, the
        // document is unusable, and the one thing that must not happen is
        // overwriting it with a default list, so the store refuses writes and
        // `get` reports why.
        if let Err(reason) = normalize(store.read().settings) {
            store.refuse_writes(reason);
        }
        Self { store }
    }

    pub(crate) fn get(&self) -> Result<ExternalAppSettings, String> {
        if let Some(reason) = self.store.refusal() {
            return Err(reason.to_owned());
        }
        Ok(self.store.read().settings)
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
        // Durable before visible: a rejected or refused write must not leave the
        // caller believing the list is saved.
        let stored = settings.clone();
        self.store
            .update(|document| document.settings = stored)
            .map_err(|error| error.to_string())?;
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
    use std::fs;

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

    #[test]
    fn a_corrupt_file_is_kept_and_the_list_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("external-apps.json");
        fs::write(&path, b"{ truncated").unwrap();

        let manager = ExternalAppManager::load(path.clone());

        assert_eq!(manager.get().unwrap(), ExternalAppSettings::default());
        assert_eq!(
            fs::read(dir.path().join("external-apps.json.corrupt")).unwrap(),
            b"{ truncated".as_slice()
        );
        // Recovered, not stuck: the next save writes a real file again.
        assert!(!path.exists());
        manager.update(ExternalAppSettings::default()).unwrap();
        assert!(path.is_file());
    }

    #[test]
    fn a_newer_file_is_refused_and_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("external-apps.json");
        let original = br#"{"version": 9, "apps": [], "defaultAppId": null}"#;
        fs::write(&path, original).unwrap();

        let manager = ExternalAppManager::load(path.clone());

        assert!(manager.get().is_err(), "a newer file is not an empty list");
        assert!(
            manager.update(ExternalAppSettings::default()).is_err(),
            "an older build must never rewrite a newer file"
        );
        assert_eq!(fs::read(&path).unwrap(), original.as_slice());
    }

    #[test]
    fn a_document_that_fails_validation_is_never_replaced_by_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("external-apps.json");
        // Valid JSON, unusable document: two entries share one id.
        let duplicated = ExternalAppSettings {
            apps: vec![
                ExternalApplication {
                    id: "a".into(),
                    name: "A".into(),
                    executable_path: dir.path().join("a"),
                },
                ExternalApplication {
                    id: "a".into(),
                    name: "B".into(),
                    executable_path: dir.path().join("b"),
                },
            ],
            default_app_id: Some("a".into()),
        };
        let original = serde_json::to_vec(&PersistedSettings {
            version: SETTINGS_VERSION,
            settings: duplicated,
        })
        .unwrap();
        fs::write(&path, &original).unwrap();

        let manager = ExternalAppManager::load(path.clone());

        assert!(manager.get().is_err());
        assert!(manager.update(ExternalAppSettings::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}
