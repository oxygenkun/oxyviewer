//! Verified local artifact import. Download transports can stream to a temporary
//! source file and use the same verification and atomic installation boundary.

use crate::{PipelineError, require_installable};
use oxy_domain::{PersonModelArtifact, PipelineManifest};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::NamedTempFile;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArtifactInstallError {
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    #[error("model artifact is not in the trusted pipeline manifest")]
    MissingArtifact,
    #[error("model artifact length or SHA-256 does not match the trusted manifest")]
    InvalidContent,
    #[error("installed model artifact is corrupt")]
    CorruptInstalledArtifact,
    #[error("model artifact installation was cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("model artifact download failed: {0}")]
    Network(#[from] ureq::Error),
}

fn verify_reader(
    artifact: &PersonModelArtifact,
    input: impl Read,
    output: Option<&mut File>,
    progress: &mut impl FnMut(u64, u64) -> bool,
) -> Result<(), ArtifactInstallError> {
    verify_content(
        artifact.size_bytes,
        &artifact.sha256,
        input,
        output,
        progress,
    )
}

pub(crate) fn verify_content(
    size_bytes: u64,
    sha256: &str,
    mut input: impl Read,
    mut output: Option<&mut File>,
    progress: &mut impl FnMut(u64, u64) -> bool,
) -> Result<(), ArtifactInstallError> {
    let mut digest = Sha256::new();
    let mut count = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if !progress(count, size_bytes) {
            return Err(ArtifactInstallError::Cancelled);
        }
        let size = input.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        count = count
            .checked_add(size as u64)
            .ok_or(ArtifactInstallError::InvalidContent)?;
        if count > size_bytes {
            return Err(ArtifactInstallError::InvalidContent);
        }
        digest.update(&buffer[..size]);
        if let Some(writer) = output.as_mut() {
            writer.write_all(&buffer[..size])?;
        }
    }
    if count != size_bytes || format!("{:x}", digest.finalize()) != sha256.to_ascii_lowercase() {
        return Err(ArtifactInstallError::InvalidContent);
    }
    if !progress(count, size_bytes) {
        return Err(ArtifactInstallError::Cancelled);
    }
    Ok(())
}

fn artifact_in_manifest<'a>(
    manifest: &'a PipelineManifest,
    artifact_id: &str,
) -> Result<&'a PersonModelArtifact, ArtifactInstallError> {
    require_installable(manifest)?;
    manifest
        .stages
        .iter()
        .filter_map(|stage| stage.artifact.as_ref())
        .find(|artifact| artifact.artifact_id == artifact_id)
        .ok_or(ArtifactInstallError::MissingArtifact)
}

/// Returns the verified installed file, if present. Corrupt files are reported
/// rather than reused or silently removed.
pub fn installed_artifact(
    manifest: &PipelineManifest,
    artifact_id: &str,
    store_dir: &Path,
) -> Result<Option<PathBuf>, ArtifactInstallError> {
    let artifact = artifact_in_manifest(manifest, artifact_id)?;
    let destination = store_dir.join(artifact.sha256.to_ascii_lowercase());
    if !destination.exists() {
        return Ok(None);
    }
    match verify_reader(artifact, File::open(&destination)?, None, &mut |_, _| true) {
        Ok(()) => Ok(Some(destination)),
        Err(ArtifactInstallError::InvalidContent) => {
            Err(ArtifactInstallError::CorruptInstalledArtifact)
        }
        Err(error) => Err(error),
    }
}

/// Streams a user-triggered download or offline import into a temporary file,
/// then verifies and atomically installs it. `progress` returns false to cancel.
/// The caller supplies an application-shipped manifest and an app-owned store.
pub fn install_artifact_reader(
    manifest: &PipelineManifest,
    artifact_id: &str,
    input: impl Read,
    store_dir: &Path,
    progress: &mut impl FnMut(u64, u64) -> bool,
) -> Result<PathBuf, ArtifactInstallError> {
    let artifact = artifact_in_manifest(manifest, artifact_id)?;
    if let Some(installed) = installed_artifact(manifest, artifact_id, store_dir)? {
        return Ok(installed);
    }
    std::fs::create_dir_all(store_dir)?;
    let destination = store_dir.join(artifact.sha256.to_ascii_lowercase());
    let mut temporary = NamedTempFile::new_in(store_dir)?;
    verify_reader(artifact, input, Some(temporary.as_file_mut()), progress)?;
    temporary.as_file_mut().sync_all()?;
    match temporary.persist_noclobber(&destination) {
        Ok(_) => Ok(destination),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            installed_artifact(manifest, artifact_id, store_dir)?
                .ok_or(ArtifactInstallError::CorruptInstalledArtifact)
        }
        Err(error) => Err(ArtifactInstallError::Io(error.error)),
    }
}

/// Installs a user-selected local file under its digest. The caller must supply
/// an application-owned store directory and an application-shipped manifest.
/// No model code is executed while importing the artifact.
pub fn install_local_artifact(
    manifest: &PipelineManifest,
    artifact_id: &str,
    source: &Path,
    store_dir: &Path,
) -> Result<PathBuf, ArtifactInstallError> {
    if let Some(installed) = installed_artifact(manifest, artifact_id, store_dir)? {
        return Ok(installed);
    }
    install_artifact_reader(
        manifest,
        artifact_id,
        File::open(source)?,
        store_dir,
        &mut |_, _| true,
    )
}

/// Called only from an explicit user action with an application-shipped
/// manifest. HTTPS is required for the initial request and every redirect.
/// The body is streamed through the same bounded verifier as offline import.
pub fn download_artifact(
    manifest: &PipelineManifest,
    artifact_id: &str,
    store_dir: &Path,
    progress: &mut impl FnMut(u64, u64) -> bool,
) -> Result<PathBuf, ArtifactInstallError> {
    let artifact = artifact_in_manifest(manifest, artifact_id)?;
    if let Some(installed) = installed_artifact(manifest, artifact_id, store_dir)? {
        return Ok(installed);
    }
    let agent = ureq::config::Config::builder()
        .https_only(true)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(20 * 60)))
        .build()
        .new_agent();
    let mut response = agent
        .get(&artifact.download_url)
        .header("User-Agent", "OxyViewer person model installer")
        .call()?;
    install_artifact_reader(
        manifest,
        artifact_id,
        response.body_mut().as_reader(),
        store_dir,
        progress,
    )
}
