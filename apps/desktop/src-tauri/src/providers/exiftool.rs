use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

const VERSION: &str = "13.59";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const SOURCE_URL: &str =
    "https://sourceforge.net/projects/exiftool/files/Image-ExifTool-13.59.tar.gz/download";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const SOURCE_SHA256: &str = "668ea3acececb7235fbd0f4900e72d5f12c9b07e5c778fd36cb1e9b5828fd65a";
#[cfg(windows)]
const WINDOWS_X64_URL: &str =
    "https://sourceforge.net/projects/exiftool/files/exiftool-13.59_64.zip/download";
#[cfg(windows)]
const WINDOWS_X64_SHA256: &str = "44b512b25af500724ba579d0a53c8fc5851628b692dd5e5d94ae4a15c2cba9ec";

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user_executable: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExiftoolStatus {
    pub available: bool,
    pub source: Option<&'static str>,
    pub version: Option<String>,
    pub executable_path: Option<PathBuf>,
    pub detail: Option<String>,
}

pub struct ProviderManager {
    data_dir: PathBuf,
    facade: oxy_metadata::MetadataFacade,
}

impl ProviderManager {
    pub fn load(data_dir: PathBuf) -> Self {
        let user = read_config(&data_dir).and_then(|config| config.user_executable);
        let managed = managed_executable(&data_dir).filter(|path| path.is_file());
        let configured = user.filter(|path| path.is_file()).or(managed);
        Self {
            data_dir,
            facade: oxy_metadata::MetadataFacade::new(configured),
        }
    }

    pub fn facade(&self) -> oxy_metadata::MetadataFacade {
        self.facade.clone()
    }

    pub fn status(&self) -> ExiftoolStatus {
        let executable = self.facade.exiftool();
        match self.facade.exiftool_version() {
            Ok(version) => ExiftoolStatus {
                available: true,
                source: Some(if executable.is_some() {
                    "configured"
                } else {
                    "path"
                }),
                version: Some(version),
                executable_path: executable,
                detail: None,
            },
            Err(error) => ExiftoolStatus {
                available: false,
                source: None,
                version: None,
                executable_path: executable,
                detail: Some(error.to_string()),
            },
        }
    }

    pub fn set_user_executable(&self, path: PathBuf) -> Result<ExiftoolStatus, String> {
        let version =
            oxy_metadata::probe_exiftool(Some(&path)).map_err(|error| error.to_string())?;
        write_config(
            &self.data_dir,
            &ProviderConfig {
                user_executable: Some(path.clone()),
            },
        )?;
        self.facade.set_exiftool(Some(path.clone()));
        Ok(ExiftoolStatus {
            available: true,
            source: Some("user"),
            version: Some(version),
            executable_path: Some(path),
            detail: None,
        })
    }

    pub fn install_managed(&self) -> Result<ExiftoolStatus, String> {
        let executable = managed_executable(&self.data_dir)
            .ok_or_else(|| "managed ExifTool is not available for this platform".to_owned())?;
        if !executable.is_file() {
            download_and_extract(&self.data_dir)?;
        }
        let version = oxy_metadata::probe_exiftool(Some(&executable))
            .map_err(|error| format!("downloaded ExifTool failed validation: {error}"))?;
        self.facade.set_exiftool(Some(executable.clone()));
        Ok(ExiftoolStatus {
            available: true,
            source: Some("managed"),
            version: Some(version),
            executable_path: Some(executable),
            detail: None,
        })
    }
}

fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("metadata-provider.json")
}

fn read_config(data_dir: &Path) -> Option<ProviderConfig> {
    let bytes = fs::read(config_path(data_dir)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_config(data_dir: &Path, config: &ProviderConfig) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    let destination = config_path(data_dir);
    let temporary = destination.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(temporary, destination).map_err(|error| error.to_string())
}

fn install_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("providers").join("exiftool").join(VERSION)
}

#[cfg(windows)]
fn managed_executable(data_dir: &Path) -> Option<PathBuf> {
    Some(install_dir(data_dir).join("exiftool.exe"))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn managed_executable(data_dir: &Path) -> Option<PathBuf> {
    Some(
        install_dir(data_dir)
            .join(format!("Image-ExifTool-{VERSION}"))
            .join("exiftool"),
    )
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn managed_executable(_data_dir: &Path) -> Option<PathBuf> {
    None
}

fn download(url: &str, expected_sha256: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::config::Config::builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        )
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        .header("User-Agent", "OxyViewer metadata provider installer")
        .call()
        .map_err(|error| format!("ExifTool download failed: {error}"))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("ExifTool download failed: {error}"))?;
    verify_checksum(&bytes, expected_sha256)?;
    Ok(bytes)
}

fn verify_checksum(bytes: &[u8], expected_sha256: &str) -> Result<(), String> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != expected_sha256 {
        return Err(format!(
            "ExifTool checksum mismatch: expected {expected_sha256}, received {actual}"
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn download_and_extract(data_dir: &Path) -> Result<(), String> {
    use flate2::read::GzDecoder;

    let bytes = download(SOURCE_URL, SOURCE_SHA256)?;
    let destination = install_dir(data_dir);
    let parent = destination
        .parent()
        .ok_or_else(|| "invalid ExifTool install directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let staging = parent.join(format!("{VERSION}.installing-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
    }
    fs::create_dir(&staging).map_err(|error| error.to_string())?;
    let result = tar::Archive::new(GzDecoder::new(Cursor::new(bytes)))
        .unpack(&staging)
        .map_err(|error| format!("could not extract ExifTool: {error}"));
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if destination.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "ExifTool install directory already exists: {}",
            destination.display()
        ));
    }
    fs::rename(&staging, &destination).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = managed_executable(data_dir).expect("supported platform");
        let mut permissions = fs::metadata(&executable)
            .map_err(|error| error.to_string())?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(windows)]
fn download_and_extract(data_dir: &Path) -> Result<(), String> {
    let bytes = download(WINDOWS_X64_URL, WINDOWS_X64_SHA256)?;
    let destination = install_dir(data_dir);
    let parent = destination
        .parent()
        .ok_or_else(|| "invalid ExifTool install directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let staging = parent.join(format!("{VERSION}.installing-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
    }
    fs::create_dir(&staging).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("could not open ExifTool archive: {error}"))?;
    archive
        .extract(&staging)
        .map_err(|error| format!("could not extract ExifTool: {error}"))?;
    let packaged = staging.join("exiftool(-k).exe");
    fs::rename(packaged, staging.join("exiftool.exe")).map_err(|error| error.to_string())?;
    if destination.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "ExifTool install directory already exists: {}",
            destination.display()
        ));
    }
    fs::rename(staging, destination).map_err(|error| error.to_string())
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn download_and_extract(_data_dir: &Path) -> Result<(), String> {
    Err("managed ExifTool is not available for this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_path_is_versioned() {
        let path = managed_executable(Path::new("/application-data")).unwrap();
        assert!(path.to_string_lossy().contains(VERSION));
    }

    #[test]
    fn config_round_trips_unicode_path() {
        let temp = tempfile::tempdir().unwrap();
        let expected = PathBuf::from("/工具/元数据/exiftool");
        write_config(
            temp.path(),
            &ProviderConfig {
                user_executable: Some(expected.clone()),
            },
        )
        .unwrap();
        assert_eq!(
            read_config(temp.path()).unwrap().user_executable,
            Some(expected)
        );
    }

    #[test]
    fn rejects_download_with_unexpected_checksum() {
        let error = verify_checksum(
            b"not an ExifTool archive",
            "668ea3acececb7235fbd0f4900e72d5f12c9b07e5c778fd36cb1e9b5828fd65a",
        )
        .unwrap_err();
        assert!(error.contains("checksum mismatch"));
    }

    #[test]
    #[ignore = "downloads ExifTool and performs a HIF embedded-XMP round trip"]
    fn installs_and_round_trips_hif_embedded_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ProviderManager::load(temp.path().to_path_buf());
        let status = manager.install_managed().unwrap();
        assert!(status.available);
        assert_eq!(status.source, Some("managed"));
        assert_eq!(status.version.as_deref(), Some(VERSION));

        let fixture = std::env::var_os("OXY_HIF_FIXTURE").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/DSC00449.HIF"),
            std::path::PathBuf::from,
        );
        if !fixture.is_file() {
            eprintln!("skipping: Sony HIF fixture unavailable (set OXY_HIF_FIXTURE)");
            return;
        }
        let hif = temp.path().join("round-trip.HIF");
        fs::copy(fixture, &hif).unwrap();
        let facade = manager.facade();
        facade
            .patch_metadata(
                &hif,
                oxy_domain::AssetKind::Heif,
                &oxy_domain::MetadataPatch {
                    rating: Some(Some(4)),
                    color_label: Some(Some("Blue".into())),
                    ..oxy_domain::MetadataPatch::default()
                },
            )
            .unwrap();
        facade.sync_metadata_to_embedded(&hif).unwrap();
        fs::remove_file(oxy_fs::sidecar_path(&hif)).unwrap();
        let embedded = facade
            .read_metadata(&hif, oxy_domain::AssetKind::Heif)
            .unwrap();
        assert_eq!(embedded.rating, Some(4));
        assert_eq!(embedded.color_label.as_deref(), Some("Blue"));
    }
}
