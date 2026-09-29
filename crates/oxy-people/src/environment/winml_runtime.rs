//! Pinned Windows ML runtime, downloaded only by an explicit install action.
//! Extract only named, independently verified DLLs into an atomic version dir.

use crate::environment::artifacts::verify_content;
use std::{
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};

pub const VERSION: &str = "winml-2.4.89-x64";
pub const SIZE: u64 = 51_594_185;
pub const URL: &str = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.ai.machinelearning/2.4.89/microsoft.windows.ai.machinelearning.2.4.89.nupkg";
pub const RUNTIME_SHA: &str = "6c916621c1807eba630388868bff8eb4cc9718b9fa17464fc9368eb14ad0f33e";
pub const DML_SHA: &str = "4a263e851f513f7c60a2b5bb08c3ed92102e3007f241c6ca5d63abf29bf04358";
const FILES: [(&str, u64, &str); 3] = [
    ("onnxruntime.dll", 23_086_440, RUNTIME_SHA),
    ("DirectML.dll", 18_700_264, DML_SHA),
    (
        "Microsoft.Windows.AI.MachineLearning.dll",
        971_504,
        "b401a47552a745755d65100828faefcf3df3895c759f96ac625ddf990711c8dd",
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
    let mut archive = tempfile::NamedTempFile::new_in(root).map_err(|error| error.to_string())?;
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
    let mut response = agent.get(URL).call().map_err(|error| error.to_string())?;
    verify_content(
        SIZE,
        "5c68ecfb947223267abf159a023f5192ad42725e4e9cc995e7c1470dd54dff63",
        response.body_mut().as_reader(),
        Some(archive.as_file_mut()),
        progress,
    )
    .map_err(|error| error.to_string())?;
    let mut zip = zip::ZipArchive::new(archive.reopen().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let temporary = tempfile::tempdir_in(root).map_err(|error| error.to_string())?;
    for (name, size, digest) in FILES {
        let member = zip
            .by_name(&format!("runtimes/win-x64/native/{name}"))
            .map_err(|error| error.to_string())?;
        let mut output =
            File::create(temporary.path().join(name)).map_err(|error| error.to_string())?;
        verify_content(size, digest, member, Some(&mut output), &mut |_, _| {
            progress(SIZE, SIZE)
        })
        .map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
    }
    if !progress(SIZE, SIZE) {
        return Err("已取消".into());
    }
    std::fs::rename(temporary.path(), root.join(VERSION)).map_err(|error| error.to_string())?;
    Ok(())
}
