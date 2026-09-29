//! Pinned standalone ONNX Runtime DirectML and its DirectML dependency, downloaded only by an explicit install action.
//! Extract only named, independently verified DLLs into an atomic version dir.

use crate::environment::artifacts::verify_content;
use std::{
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};

pub const VERSION: &str = "ort-directml-1.24.4-dml-1.15.4-x64";
pub const SIZE: u64 = 12_458_649 + 202_292_617;
pub const SOURCE_URL: &str =
    "https://www.nuget.org/packages/Microsoft.ML.OnnxRuntime.DirectML/1.24.4";
pub const RUNTIME_SHA: &str = "e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40";
pub const DML_SHA: &str = "9c9e6d822561c6c41b90e6994b3e8857cf1d66dbfb1e0c4c799c7c89b4e92da1";
const FILES: [(&str, u64, &str); 3] = [
    ("onnxruntime.dll", 17_328_152, RUNTIME_SHA),
    (
        "onnxruntime_providers_shared.dll",
        22_040,
        "265c8daf29637cb259cac8be9f08f2cd45f3883f0f0e4949cbfddd5b4cbec3b6",
    ),
    ("DirectML.dll", 18_527_776, DML_SHA),
];
const PACKAGES: [(&str, u64, &str, &str, &[usize]); 2] = [
    (
        "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/1.24.4/microsoft.ml.onnxruntime.directml.1.24.4.nupkg",
        12_458_649,
        "57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681",
        "runtimes/win-x64/native",
        &[0, 1],
    ),
    (
        "https://api.nuget.org/v3-flatcontainer/microsoft.ai.directml/1.15.4/microsoft.ai.directml.1.15.4.nupkg",
        202_292_617,
        "4e7cb7ddce8cf837a7a75dc029209b520ca0101470fcdf275c1f49736a3615b9",
        "bin/x64-win",
        &[2],
    ),
];

pub fn installed(root: &Path) -> Result<Option<PathBuf>, String> {
    let directory = root.join(VERSION);
    if !directory.exists() {
        return Ok(None);
    }
    for (name, size, digest) in FILES {
        verify_content(
            size,
            digest,
            File::open(directory.join(name)).map_err(|error| error.to_string())?,
            None,
            &mut |_, _| true,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(Some(directory.join("onnxruntime.dll")))
}

pub fn download(root: &Path, progress: &mut impl FnMut(u64, u64) -> bool) -> Result<(), String> {
    if installed(root)?.is_some() {
        return Ok(());
    }
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
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
        .timeout_recv_body(Some(Duration::from_secs(1200)))
        .build()
        .new_agent();
    let temporary = tempfile::tempdir_in(root).map_err(|error| error.to_string())?;
    let mut completed = 0;
    for (url, size, sha, prefix, indices) in PACKAGES {
        if !progress(completed, SIZE) {
            return Err("已取消".into());
        }
        let mut archive = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
        let mut response = agent.get(url).call().map_err(|e| e.to_string())?;
        verify_content(
            size,
            sha,
            response.body_mut().as_reader(),
            Some(archive.as_file_mut()),
            &mut |bytes, _| progress(completed + bytes, SIZE),
        )
        .map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(archive.reopen().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        for &index in indices {
            let (name, file_size, digest) = FILES[index];
            let member = zip
                .by_name(&format!("{prefix}/{name}"))
                .map_err(|e| e.to_string())?;
            let mut output =
                File::create(temporary.path().join(name)).map_err(|e| e.to_string())?;
            verify_content(file_size, digest, member, Some(&mut output), &mut |_, _| {
                progress(completed + size, SIZE)
            })
            .map_err(|e| e.to_string())?;
            output.sync_all().map_err(|e| e.to_string())?;
        }
        // The archive digest above authenticates these upstream notices too.
        let (component, license) = if indices.contains(&0) {
            ("onnxruntime", "LICENSE")
        } else {
            ("DirectML", "LICENSE.txt")
        };
        for (member_name, suffix) in [
            (license, "LICENSE.txt"),
            ("ThirdPartyNotices.txt", "ThirdPartyNotices.txt"),
        ] {
            let mut member = zip.by_name(member_name).map_err(|e| e.to_string())?;
            let mut output = File::create(temporary.path().join(format!("{component}-{suffix}")))
                .map_err(|e| e.to_string())?;
            std::io::copy(&mut member, &mut output).map_err(|e| e.to_string())?;
            output.sync_all().map_err(|e| e.to_string())?;
        }
        completed += size;
    }
    if !progress(SIZE, SIZE) {
        return Err("已取消".into());
    }
    std::fs::rename(temporary.path(), root.join(VERSION)).map_err(|error| error.to_string())?;
    Ok(())
}
