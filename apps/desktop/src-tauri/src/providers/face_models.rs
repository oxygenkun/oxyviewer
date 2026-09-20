//! User-initiated installation of optional face-analysis models.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use oxy_domain::{FaceModelDownloadProgress, FaceModelDownloadStage, FaceModelStatus};
use sha2::{Digest, Sha256};

pub const FACE_MODEL_DOWNLOAD_PROGRESS_EVENT: &str = "face-model-download-progress";

#[derive(Debug, Clone, Copy)]
pub enum ManagedFaceModel {
    AdaFace,
    Scrfd,
}

impl ManagedFaceModel {
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "adaface-ir101" => Some(Self::AdaFace),
            "scrfd-10g-kps" => Some(Self::Scrfd),
            _ => None,
        }
    }

    fn file(self) -> &'static str {
        self.spec().file.as_str()
    }

    fn digest(self) -> &'static str {
        self.spec().sha256.as_str()
    }

    fn size(self) -> u64 {
        self.spec().size_bytes
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::AdaFace => "adaface-ir101",
            Self::Scrfd => "scrfd-10g-kps",
        }
    }

    pub fn download_size(self) -> u64 {
        self.spec()
            .archive_size_bytes
            .unwrap_or_else(|| self.size())
    }

    fn spec(self) -> &'static oxy_faces::ManagedModelSpec {
        oxy_faces::managed_model_manifest()
            .models
            .iter()
            .find(|model| model.id == self.id())
            .expect("managed model id must exist in the checked-in manifest")
    }
}

pub fn model_directory(data_dir: &Path) -> PathBuf {
    data_dir.join("face-models")
}

pub fn statuses(data_dir: &Path) -> Vec<FaceModelStatus> {
    let directory = model_directory(data_dir);
    [ManagedFaceModel::Scrfd, ManagedFaceModel::AdaFace]
        .into_iter()
        .map(|model| FaceModelStatus {
            id: model.id().into(),
            display_name: model.spec().display_name.clone(),
            installed: is_verified(&directory, model),
            size_bytes: model.size(),
            download_size_bytes: model.download_size(),
            license_summary: model.spec().license_summary.clone(),
        })
        .collect()
}

pub fn installed_pair(data_dir: &Path) -> Option<oxy_faces::FaceModelPaths> {
    let directory = model_directory(data_dir);
    if !is_verified(&directory, ManagedFaceModel::Scrfd)
        || !is_verified(&directory, ManagedFaceModel::AdaFace)
    {
        return None;
    }
    oxy_faces::managed_model_paths(&directory)
}

pub fn install(
    data_dir: &Path,
    model: ManagedFaceModel,
    mut progress: impl FnMut(FaceModelDownloadProgress),
) -> Result<(), String> {
    let directory = model_directory(data_dir);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    match model {
        ManagedFaceModel::AdaFace => {
            let spec = model.spec();
            download_file(
                spec.url.as_deref().expect("AdaFace URL is required"),
                &spec.sha256,
                &directory.join(model.file()),
                model,
                &mut progress,
            )?;
        }
        ManagedFaceModel::Scrfd => install_scrfd(&directory, &mut progress)?,
    }
    write_marker(&directory, model)?;
    progress(model_progress(
        model,
        FaceModelDownloadStage::Complete,
        model.download_size(),
        None,
    ));
    Ok(())
}

fn install_scrfd(
    directory: &Path,
    progress: &mut dyn FnMut(FaceModelDownloadProgress),
) -> Result<(), String> {
    let model = ManagedFaceModel::Scrfd;
    let spec = model.spec();
    let archive_path = directory.join(format!("buffalo_l-{}.zip", std::process::id()));
    let result = (|| {
        download_file(
            spec.archive_url
                .as_deref()
                .expect("SCRFD archive URL is required"),
            spec.archive_sha256
                .as_deref()
                .expect("SCRFD archive digest is required"),
            &archive_path,
            model,
            progress,
        )?;
        progress(model_progress(
            model,
            FaceModelDownloadStage::Installing,
            model.download_size(),
            None,
        ));
        let archive_file = File::open(&archive_path).map_err(|error| error.to_string())?;
        let mut archive = zip::ZipArchive::new(archive_file)
            .map_err(|error| format!("invalid SCRFD model archive: {error}"))?;
        let mut model_index = None;
        for index in 0..archive.len() {
            let name = archive
                .by_index(index)
                .map_err(|error| format!("invalid SCRFD model archive: {error}"))?
                .name()
                .to_owned();
            let entry = spec
                .archive_entry
                .as_deref()
                .expect("SCRFD archive entry is required");
            if name == entry || name.ends_with(&format!("/{entry}")) {
                model_index = Some(index);
                break;
            }
        }
        let mut source = archive
            .by_index(model_index.ok_or("SCRFD model is missing from archive")?)
            .map_err(|error| format!("SCRFD model is missing from archive: {error}"))?;
        let target = directory.join(model.file());
        let staging = target.with_extension("onnx.installing");
        let mut output = File::create(&staging).map_err(|error| error.to_string())?;
        std::io::copy(&mut source, &mut output).map_err(|error| error.to_string())?;
        output.flush().map_err(|error| error.to_string())?;
        verify_file(&staging, model.digest())?;
        atomic_replace(&staging, &target)
    })();
    let _ = fs::remove_file(&archive_path);
    result
}

fn download_file(
    url: &str,
    expected: &str,
    target: &Path,
    model: ManagedFaceModel,
    progress: &mut dyn FnMut(FaceModelDownloadProgress),
) -> Result<(), String> {
    let staging = target.with_extension("download");
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
        .header("User-Agent", "OxyViewer face model installer")
        .call()
        .map_err(|error| format!("model download failed: {error}"))?;
    let mut input = response.body_mut().as_reader();
    let mut output = File::create(&staging).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let total = model.download_size();
    let mut downloaded = 0u64;
    let mut last_update = std::time::Instant::now();
    progress(model_progress(
        model,
        FaceModelDownloadStage::Downloading,
        0,
        None,
    ));
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| format!("model download failed: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(|error| error.to_string())?;
        downloaded = downloaded.saturating_add(read as u64);
        if downloaded >= total || last_update.elapsed() >= std::time::Duration::from_millis(150) {
            progress(model_progress(
                model,
                FaceModelDownloadStage::Downloading,
                downloaded.min(total),
                None,
            ));
            last_update = std::time::Instant::now();
        }
    }
    output.flush().map_err(|error| error.to_string())?;
    progress(model_progress(
        model,
        FaceModelDownloadStage::Verifying,
        downloaded.min(total),
        None,
    ));
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        let _ = fs::remove_file(&staging);
        return Err(format!(
            "model checksum mismatch: expected {expected}, received {actual}"
        ));
    }
    atomic_replace(&staging, target)
}

fn model_progress(
    model: ManagedFaceModel,
    stage: FaceModelDownloadStage,
    downloaded_bytes: u64,
    message: Option<String>,
) -> FaceModelDownloadProgress {
    FaceModelDownloadProgress {
        model_id: model.id().to_owned(),
        stage,
        downloaded_bytes,
        total_bytes: model.download_size(),
        message,
    }
}

fn atomic_replace(staging: &Path, target: &Path) -> Result<(), String> {
    if target.exists() {
        fs::remove_file(target).map_err(|error| error.to_string())?;
    }
    fs::rename(staging, target).map_err(|error| error.to_string())
}

fn marker(directory: &Path, model: ManagedFaceModel) -> PathBuf {
    directory.join(format!("{}.sha256", model.file()))
}
fn write_marker(directory: &Path, model: ManagedFaceModel) -> Result<(), String> {
    fs::write(marker(directory, model), model.digest()).map_err(|error| error.to_string())
}
fn is_verified(directory: &Path, model: ManagedFaceModel) -> bool {
    fs::metadata(directory.join(model.file())).is_ok_and(|metadata| metadata.len() == model.size())
        && fs::read_to_string(marker(directory, model)).is_ok_and(|value| value == model.digest())
}
fn verify_file(path: &Path, expected: &str) -> Result<(), String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|error| error.to_string())?;
    let actual = format!("{:x}", hasher.finalize());
    (actual == expected)
        .then_some(())
        .ok_or_else(|| format!("model checksum mismatch: expected {expected}, received {actual}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_requires_file_and_matching_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let model = ManagedFaceModel::Scrfd;
        assert!(!is_verified(temp.path(), model));
        fs::write(
            temp.path().join(model.file()),
            vec![0; model.size() as usize],
        )
        .unwrap();
        assert!(!is_verified(temp.path(), model));
        write_marker(temp.path(), model).unwrap();
        assert!(is_verified(temp.path(), model));
    }
}
