//! Minimal end-to-end benchmark for HEIF thumbnail, loupe, and full tile display.
//!
//! Run from the workspace root:
//! `cargo run --release -p oxy-media --bin heif_display_bench -- tests/fixtures/DSC00449.HIF 5`

use oxy_media::{HeifDecodeService, heif_preview};
use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn Error>> {
    let path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tests/fixtures/DSC00449.HIF"));
    let runs = env::args()
        .nth(2)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(3);

    println!("file={} runs={runs}", path.display());
    benchmark_preview(&path, 512, runs)?;
    benchmark_preview(&path, 4_096, runs)?;
    benchmark_tiles(&path, runs)?;
    Ok(())
}

fn benchmark_preview(path: &Path, size: u32, runs: usize) -> Result<(), Box<dyn Error>> {
    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let cache = tempfile::tempdir()?;
        let started = Instant::now();
        let preview = heif_preview(path, cache.path(), size)?;
        samples.push(started.elapsed());
        let longest_edge = preview.width.max(preview.height);
        assert!(longest_edge <= size && longest_edge >= size.saturating_sub(2));
    }
    print_samples(&format!("{size}px preview"), &mut samples);
    Ok(())
}

fn benchmark_tiles(path: &Path, runs: usize) -> Result<(), Box<dyn Error>> {
    let mut first_tile = Vec::with_capacity(runs);
    let mut totals = Vec::with_capacity(runs);
    for generation in 0..runs {
        let service = HeifDecodeService::default();
        let session = service.begin(path, generation as u64, true)?;
        let started = Instant::now();
        let mut first = None;
        let diagnostics = service.decode(&session, path.to_owned(), |_| {
            first.get_or_insert_with(|| started.elapsed());
        })?;
        first_tile.push(first.unwrap_or_else(|| started.elapsed()));
        totals.push(started.elapsed());
        println!(
            "  backend={:?} acceleration={:?} decode={}ms publish={}ms",
            diagnostics.backend,
            diagnostics.acceleration,
            diagnostics.decode_ms,
            diagnostics.tile_publish_ms
        );
    }
    print_samples("full first tile", &mut first_tile);
    print_samples("full all tiles", &mut totals);
    Ok(())
}

fn print_samples(name: &str, samples: &mut [Duration]) {
    samples.sort();
    let median = samples[samples.len() / 2];
    let min = samples[0];
    println!("{name:18} median={median:.2?} min={min:.2?}");
}
