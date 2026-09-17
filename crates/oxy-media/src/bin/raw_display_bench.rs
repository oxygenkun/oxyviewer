//! End-to-end benchmark for the RAW loupe pipeline.
//!
//! Run from the workspace root:
//! `cargo run --release -p oxy-media --bin raw_display_bench -- tests/fixtures/DSC00529.ARW`

use oxy_domain::{AssetKind, PreviewPriority, RenderLevel};
use oxy_media::{dimensions, preview};
use oxy_runtime::CancellationToken;
use std::{env, error::Error, path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let path = env::args_os().nth(1).map_or_else(
        || PathBuf::from("tests/fixtures/DSC00529.ARW"),
        PathBuf::from,
    );
    let include_full = env::args().any(|argument| argument == "--full");

    let source = dimensions(&path, AssetKind::Raw)?;
    println!(
        "file={} source={}x{}",
        path.display(),
        source.width,
        source.height
    );

    let cache = tempfile::tempdir()?;
    for (size, level) in [(512, RenderLevel::Thumbnail), (4_096, RenderLevel::Preview)] {
        let started = Instant::now();
        let preview = preview(
            &path,
            cache.path(),
            level,
            PreviewPriority::Preload,
            AssetKind::Raw,
            &CancellationToken::default(),
        )?;
        println!(
            "{size}px preview: {}x{} {:.1}KiB kind={:?} elapsed={:.2?}",
            preview.width,
            preview.height,
            std::fs::metadata(&preview.path)?.len() as f64 / 1024.0,
            preview.kind,
            started.elapsed()
        );
    }

    if include_full {
        let started = Instant::now();
        let full = preview(
            &path,
            cache.path(),
            RenderLevel::Full,
            PreviewPriority::Loupe,
            AssetKind::Raw,
            &CancellationToken::default(),
        )?;
        println!(
            "full detail: {}x{} {:.1}MiB kind={:?} elapsed={:.2?}",
            full.width,
            full.height,
            std::fs::metadata(&full.path)?.len() as f64 / (1024.0 * 1024.0),
            full.kind,
            started.elapsed()
        );
    }

    Ok(())
}
