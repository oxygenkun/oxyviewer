//! Measure the public browse use case with isolated temporary application state.
//! cargo run -p oxy-core --example browse_latency -- <directory>
use oxy_core::{AppPaths, OxyApp, contracts::AssetQuery};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("directory argument required")?,
    )
    .canonicalize()?;
    let cache = tempfile::tempdir()?;
    for run in ["cold", "restart", "memory"] {
        let (app, _) = OxyApp::open(AppPaths {
            data_dir: cache.path().join("data"),
            preview_dir: cache.path().join("previews"),
        })?;
        let session = app.browse.open_folder(root.clone())?;
        let started = Instant::now();
        let read =
            app.browse
                .list_assets(session.id.clone(), None, AssetQuery::default(), None, None)?;
        println!(
            "{run}: source={} count={} enumerate_ms={} attributes_ms={} list_ms={} sort_ms={} total_ms={:.2}",
            read.progress.source,
            read.page.total,
            read.progress.enumeration_ms,
            read.progress.attributes_ms,
            read.progress.cache_ms,
            read.progress.sort_ms,
            started.elapsed().as_secs_f64() * 1000.0
        );
        if run == "memory" {
            let started = Instant::now();
            app.browse
                .list_assets(session.id, None, AssetQuery::default(), None, None)?;
            println!(
                "same-process: total_ms={:.2}",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        if !app.shutdown(Duration::from_secs(3)) {
            return Err("application workers did not finish".into());
        }
    }
    Ok(())
}
