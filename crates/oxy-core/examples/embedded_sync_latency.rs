//! Release comparison on disposable copies; never writes the supplied sources.
//! Arguments: <scratch-parent> <exiftool> <ffprobe> <HIF> [HIF ...]
//! Prefix `--concurrency` to compare 1/2/4 sessions using four copies per source.
use oxy_core::{
    AppPaths, OxyApp,
    metadata::{
        MetadataFacade, MetadataReader, NativeMetadataReader,
        types::{EmbeddedSyncOptions, MetadataPatch},
    },
};
use oxy_media::types::{AssetKind, PickLabel};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

type Error = Box<dyn std::error::Error>;

fn digest(path: &Path) -> Result<Vec<u8>, Error> {
    Ok(Sha256::digest(fs::read(path)?).to_vec())
}

fn packets(probe: &Path, path: &Path) -> Result<Value, Error> {
    let output = Command::new(probe)
        .args([
            "-v",
            "error",
            "-show_packets",
            "-show_data_hash",
            "sha256",
            "-show_entries",
            "packet=stream_index,size,data_hash",
            "-of",
            "json",
        ])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    if value["packets"].as_array().is_none_or(Vec::is_empty) {
        return Err("ffprobe returned no compressed image packets".into());
    }
    Ok(value)
}

/// Reconstructs the previous per-file writer transport. It deliberately excludes
/// old IPC, sidecar parsing and projection overhead, so it is a conservative baseline.
fn previous_writer(tool: &Path, paths: &[PathBuf], rating: u8) -> Result<f64, Error> {
    let start = Instant::now();
    for path in paths {
        let output = Command::new(tool)
            .args(["-overwrite_original", "-P", "-api", "Compact=AllFormat"])
            .arg(format!("-XMP:Rating={rating}"))
            .args([
                "-XMP:Label=blue",
                "-XMP-digiKam:PickLabel=1",
                "-XMP:Subject=",
                "-XMP:Subject+=OxyViewer probe",
                "-XMP-lr:HierarchicalSubject=",
                "-XMP-lr:HierarchicalSubject+=Probe|OxyViewer probe",
            ])
            .arg(path)
            .output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
    }
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

fn run_batch(app: &OxyApp, paths: &[PathBuf], request: &str, equal: bool) -> Result<f64, Error> {
    let start = Instant::now();
    let result = app
        .metadata_service
        .sync_to_embedded(paths.to_vec(), request.into())?
        .recv()??;
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(result.completed, paths.len());
    assert_eq!(result.updated, if equal { 0 } else { paths.len() });
    assert_eq!(result.unchanged, if equal { paths.len() } else { 0 });
    assert_eq!(result.skipped, 0);
    assert!(!result.cancelled);
    assert!(result.failures.is_empty(), "{:?}", result.failures);
    Ok(elapsed)
}

fn main() -> Result<(), Error> {
    let args = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--concurrency") {
        return concurrency_comparison(&args[1..]);
    }
    if args.len() < 4 {
        return Err("arguments: <scratch-parent> <exiftool> <ffprobe> <HIF> [HIF ...]".into());
    }
    let scratch = tempfile::Builder::new()
        .prefix(".oxyviewer-embedded-bench-")
        .tempdir_in(&args[0])?;
    let tool = &args[1];
    let probe = &args[2];
    let sources = &args[3..];
    let source_hashes = sources
        .iter()
        .map(|path| digest(path))
        .collect::<Result<Vec<_>, _>>()?;
    let source_packets = sources
        .iter()
        .map(|path| packets(probe, path))
        .collect::<Result<Vec<_>, _>>()?;
    let source_sidecars = sources
        .iter()
        .filter_map(|path| {
            let sidecar = path.with_extension("xmp");
            sidecar.is_file().then_some(sidecar)
        })
        .collect::<Vec<_>>();
    let sidecar_hashes = source_sidecars
        .iter()
        .map(|path| digest(path))
        .collect::<Result<Vec<_>, _>>()?;
    let mut trials = Vec::new();
    for round in 0..3 {
        for optimized in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let directory = scratch.path().join(format!("{round}-{optimized}"));
            fs::create_dir(&directory)?;
            let paths = sources
                .iter()
                .enumerate()
                .map(|(index, source)| {
                    let destination = directory.join(format!("photo-{index}.HIF"));
                    fs::copy(source, &destination)?;
                    Ok(destination)
                })
                .collect::<Result<Vec<_>, std::io::Error>>()?;
            let rating = round + 1;
            let patch = MetadataPatch {
                rating: Some(Some(rating)),
                color_label: Some(Some("Blue".into())),
                pick_label: Some(Some(PickLabel::Accepted)),
                keywords: Some(vec!["OxyViewer probe".into()]),
                hierarchical_keywords: Some(vec!["Probe|OxyViewer probe".into()]),
                ..MetadataPatch::default()
            };
            for path in &paths {
                MetadataFacade::default().patch_metadata(path, AssetKind::Heif, &patch)?;
            }
            let (changed_ms, equal_ms) = if optimized {
                let state = tempfile::tempdir()?;
                let (app, _events) = OxyApp::open(AppPaths::from_roots(
                    state.path().join("data"),
                    state.path().join("cache"),
                ))?;
                app.metadata_service
                    .configure_provider(tool.clone())?
                    .recv()??;
                let changed = run_batch(&app, &paths, "changed", false)?;
                for path in &paths {
                    let actual = NativeMetadataReader.read(path, None)?.editable;
                    assert_eq!(actual.rating, Some(rating));
                    assert_eq!(actual.color_label.as_deref(), Some("Blue"));
                    assert_eq!(actual.pick_label, Some(PickLabel::Accepted));
                    assert_eq!(actual.keywords, *patch.keywords.as_ref().unwrap());
                    assert_eq!(
                        actual.hierarchical_keywords,
                        *patch.hierarchical_keywords.as_ref().unwrap()
                    );
                }
                let before = paths
                    .iter()
                    .map(|path| Ok((digest(path)?, fs::metadata(path)?.modified()?)))
                    .collect::<Result<Vec<_>, Error>>()?;
                let equal = run_batch(&app, &paths, "equal", true)?;
                for (path, (hash, modified)) in paths.iter().zip(before) {
                    assert_eq!(digest(path)?, hash);
                    assert_eq!(fs::metadata(path)?.modified()?, modified);
                }
                assert!(app.shutdown(Duration::from_secs(10)));
                (changed, equal)
            } else {
                (
                    previous_writer(tool, &paths, rating)?,
                    previous_writer(tool, &paths, rating)?,
                )
            };
            for (path, expected) in paths.iter().zip(&source_packets) {
                assert_eq!(
                    &packets(probe, path)?,
                    expected,
                    "compressed image packets changed"
                );
            }
            trials.push(json!({"round":round, "optimized":optimized, "changedMs":changed_ms, "equalMs":equal_ms}));
            eprintln!(
                "round {round} optimized={optimized} changed={changed_ms:.3}ms equal={equal_ms:.3}ms"
            );
        }
    }
    for (source, expected) in sources.iter().zip(source_hashes) {
        assert_eq!(digest(source)?, expected, "source changed");
    }
    for (source, expected) in source_sidecars.iter().zip(sidecar_hashes) {
        assert_eq!(digest(source)?, expected, "source sidecar changed");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "filesPerBatch":sources.len(), "trials":trials,
            "checks": {"sourceBytesUnchanged":true, "sourceSidecarsUnchanged":true,
                "compressedImagePacketsUnchanged":true, "equalSyncBytesAndMtimeUnchanged":true}
        }))?
    );
    scratch.close()?;
    Ok(())
}

/// Compare the complete core path, with a closed-loop first-page read during
/// writes. This is backend contention coverage, not WebView interaction timing.
fn concurrency_comparison(args: &[PathBuf]) -> Result<(), Error> {
    if args.len() < 4 {
        return Err("--concurrency <scratch-parent> <exiftool> <ffprobe> <HIF> [HIF ...]".into());
    }
    let scratch = tempfile::Builder::new()
        .prefix(".oxyviewer-concurrency-core-")
        .tempdir_in(&args[0])?;
    let tool = &args[1];
    let probe = &args[2];
    let sources = &args[3..];
    let source_hashes = sources
        .iter()
        .map(|path| digest(path))
        .collect::<Result<Vec<_>, _>>()?;
    let source_packets = sources
        .iter()
        .map(|path| packets(probe, path))
        .collect::<Result<Vec<_>, _>>()?;
    let sidecars = sources
        .iter()
        .map(|path| path.with_extension("xmp"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    let sidecar_hashes = sidecars
        .iter()
        .map(|path| digest(path))
        .collect::<Result<Vec<_>, _>>()?;
    let mut trials = Vec::new();
    for (round, order) in [[1, 2, 4], [4, 1, 2], [2, 4, 1]].into_iter().enumerate() {
        for concurrency in order {
            let directory = scratch.path().join(format!("{round}-{concurrency}"));
            fs::create_dir(&directory)?;
            let mut paths = Vec::new();
            let rating = round as u8 + 1;
            let patch = MetadataPatch {
                rating: Some(Some(rating)),
                color_label: Some(Some("Blue".into())),
                pick_label: Some(Some(PickLabel::Accepted)),
                keywords: Some(vec!["OxyViewer concurrency 验证".into()]),
                hierarchical_keywords: Some(vec!["Probe|OxyViewer concurrency 验证".into()]),
                ..MetadataPatch::default()
            };
            for index in 0..sources.len() * 4 {
                let path = directory.join(format!("photo-{index:02}.HIF"));
                fs::copy(&sources[index % sources.len()], &path)?;
                MetadataFacade::default().patch_metadata(&path, AssetKind::Heif, &patch)?;
                paths.push(path);
            }
            let state = tempfile::tempdir()?;
            let (app, _events) = OxyApp::open(AppPaths::from_roots(
                state.path().join("data"),
                state.path().join("cache"),
            ))?;
            app.metadata_service
                .configure_provider(tool.clone())?
                .recv()??;
            let folder = app.browse.open_folder(directory.clone())?;
            let options = EmbeddedSyncOptions { concurrency };
            let mut browse_ms = Vec::new();
            let start = Instant::now();
            let reply = app.metadata_service.sync_to_embedded_with_options(
                paths.clone(),
                "changed".into(),
                options,
            )?;
            let result = loop {
                match reply.recv_timeout(Duration::from_millis(50)) {
                    Ok(result) => break result?,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let read = Instant::now();
                        let page = app.browse.list_assets(
                            folder.id.clone(),
                            None,
                            oxy_core::contracts::AssetQuery::default(),
                            None,
                            None,
                        )?;
                        assert_eq!(page.page.total as usize, paths.len());
                        browse_ms.push(read.elapsed().as_secs_f64() * 1000.0);
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            let changed_ms = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(result.completed, paths.len());
            assert_eq!(result.updated, paths.len());
            assert!(result.failures.is_empty(), "{:?}", result.failures);
            assert!(!result.cancelled);
            for path in &paths {
                let actual = NativeMetadataReader.read(path, None)?.editable;
                assert_eq!(actual.rating, Some(rating));
                assert_eq!(actual.color_label.as_deref(), Some("Blue"));
                assert_eq!(actual.pick_label, Some(PickLabel::Accepted));
                assert_eq!(actual.keywords, *patch.keywords.as_ref().unwrap());
                assert_eq!(
                    actual.hierarchical_keywords,
                    *patch.hierarchical_keywords.as_ref().unwrap()
                );
            }
            let before = paths
                .iter()
                .map(|path| Ok((digest(path)?, fs::metadata(path)?.modified()?)))
                .collect::<Result<Vec<_>, Error>>()?;
            let start = Instant::now();
            let equal = app
                .metadata_service
                .sync_to_embedded_with_options(paths.clone(), "equal".into(), options)?
                .recv()??;
            let equal_ms = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(equal.completed, paths.len());
            assert_eq!(equal.unchanged, paths.len());
            assert_eq!(equal.updated, 0);
            assert!(equal.failures.is_empty());
            for ((index, path), (hash, modified)) in paths.iter().enumerate().zip(before) {
                assert_eq!(digest(path)?, hash);
                assert_eq!(fs::metadata(path)?.modified()?, modified);
                assert_eq!(packets(probe, path)?, source_packets[index % sources.len()]);
            }
            assert!(app.shutdown(Duration::from_secs(10)));
            let row = json!({"round":round, "concurrency":concurrency, "changedMs":changed_ms, "equalMs":equal_ms, "browseMs":browse_ms});
            eprintln!("{row}");
            trials.push(row);
            fs::remove_dir_all(directory)?;
        }
    }
    for (path, hash) in sources.iter().zip(source_hashes) {
        assert_eq!(digest(path)?, hash);
    }
    for (path, hash) in sidecars.iter().zip(sidecar_hashes) {
        assert_eq!(digest(path)?, hash);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"filesPerBatch":sources.len()*4, "trials":trials,
        "checks":{"sourceBytesUnchanged":true,"sourceSidecarsUnchanged":true,"embeddedFieldsVerified":true,
            "equalSyncBytesAndMtimeUnchanged":true,"compressedPacketsUnchanged":true}}))?
    );
    scratch.close()?;
    Ok(())
}
