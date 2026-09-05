//! Read-only source-folder measurement using an isolated temporary database.
//! cargo run -p oxy-library --example browse_latency -- <directory>
use oxy_domain::AssetQuery;
use oxy_library::Library;
use std::{path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("directory argument required")?,
    )
    .canonicalize()?;
    let cache = tempfile::tempdir()?;
    let database = cache.path().join("browse.sqlite");
    for run in ["cold", "restart", "memory"] {
        let library = Library::open(&database)?;
        let started = Instant::now();
        let mut scan = oxy_fs::ScanProgress::default();
        let read = library.browse_directory(&root, &root, |progress| scan = progress)?;
        let list_ms = started.elapsed().as_secs_f64() * 1000.0;
        let sort = Instant::now();
        let page = oxy_fs::page_assets(&read.assets, &AssetQuery::default(), 0);
        println!(
            "{run}: source={} count={} enumerate_ms={} attributes_ms={} list_ms={list_ms:.2} sort_ms={:.2} total_ms={:.2}",
            read.source,
            page.total,
            scan.enumeration_ms,
            scan.attributes_ms,
            sort.elapsed().as_secs_f64() * 1000.0,
            started.elapsed().as_secs_f64() * 1000.0
        );
        if run == "memory" {
            let started = Instant::now();
            let read = library.browse_directory(&root, &root, |_| {})?;
            let _page = oxy_fs::page_assets(&read.assets, &AssetQuery::default(), 0);
            println!(
                "same-process: total_ms={:.2}",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
    Ok(())
}
