// Reproduce from the repository root:
// Copy this file to crates/oxy-media/examples/hif_extract_1100.rs.
// Beside it, create hif_extract_1100_support/ and copy the current production
// src/formats/heif/quirks/sony.rs and sony/geometry.rs into that directory as
// sony.rs and geometry.rs, respectively. No production source changes needed.
// cargo run --release -p oxy-media --example hif_extract_1100 -- \
//   tests/perf/generated/folder-thumbnail-retention-hif
// The directory must contain the same 1100 HIF paths used by the desktop probe.
// Remove the temporary example and support files when finished.
pub use oxy_media::{ImageDimensions, MediaError};
#[path = "hif_extract_1100_support/sony.rs"]
mod sony;
use std::{error::Error, fs, hint::black_box, path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let folder = PathBuf::from(std::env::args().nth(1).ok_or("folder required")?);
    let mut paths = fs::read_dir(folder)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("hif")));
    paths.sort();
    assert_eq!(paths.len(), 1100);
    let mut runs = Vec::new();
    for round in 1..=5 {
        let start = Instant::now();
        let mut samples = Vec::new();
        let mut total_bytes = 0;
        let mut retained = Vec::with_capacity(paths.len());
        for path in &paths {
            let item_start = Instant::now();
            let jpeg = sony::inspect(path, None)?.embedded_jpeg.ok_or("missing JPEG")?;
            total_bytes += jpeg.bytes.len();
            black_box((jpeg.width, jpeg.height, jpeg.geometry));
            retained.push(jpeg.bytes);
            samples.push(item_start.elapsed().as_secs_f64() * 1000.0);
        }
        let extract_ms = start.elapsed().as_secs_f64() * 1000.0;
        let decode_start = Instant::now();
        for bytes in &retained {
            black_box(image::load_from_memory(bytes)?);
        }
        let extra_decode_ms = decode_start.elapsed().as_secs_f64() * 1000.0;
        let first_100_ms: f64 = samples[..100].iter().sum();
        let last_100_ms: f64 = samples[1000..].iter().sum();
        samples.sort_by(f64::total_cmp);
        runs.push(serde_json::json!({"round": round, "extractMs": extract_ms,
            "additionalJpegDecodeMs": extra_decode_ms, "jpegBytes": total_bytes,
            "meanMs": extract_ms / paths.len() as f64, "p50Ms": samples[550],
            "p95Ms": samples[1045], "first100Ms": first_100_ms, "last100Ms": last_100_ms}));
    }
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({"count": paths.len(), "runs": runs}))?);
    Ok(())
}
