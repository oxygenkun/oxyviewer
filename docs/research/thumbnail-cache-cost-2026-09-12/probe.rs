use oxy_domain::{AssetKind, PreviewPriority, RenderLevel};
pub use oxy_media::{ImageDimensions, MediaError};
use oxy_media::{
    SourceRevision, preview, preview_for_app_with_completion, shared_resource_registry,
};
use oxy_runtime::CancellationToken;
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    hint::black_box,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};

#[path = "thumbnail_cache_probe/sony.rs"]
mod sony;

fn measure<T>(
    samples: &mut BTreeMap<&'static str, Vec<f64>>,
    name: &'static str,
    f: impl FnOnce() -> T,
) -> T {
    let start = Instant::now();
    let value = f();
    samples
        .entry(name)
        .or_default()
        .push(start.elapsed().as_secs_f64() * 1000.0);
    value
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = std::env::args().nth(1).expect("source HIF");
    let source = Path::new(&source);
    let runs = 30;
    let mut samples = BTreeMap::new();
    let mut byte_size = 0;
    let mut geometry = None;
    // Prototype a known byte-range locator; do not change the production parser.
    let seed = sony::inspect(source, None)?
        .embedded_jpeg
        .ok_or("no embedded jpeg")?;
    let mut original = seed.bytes.clone();
    if original.get(2..6) == Some(&[0xff, 0xe1, 0, 0x22]) {
        original.drain(2..38);
    }
    let mut prefix = Vec::new();
    fs::File::open(source)?
        .take(2 * 1024 * 1024)
        .read_to_end(&mut prefix)?;
    let offset = prefix
        .windows(original.len())
        .position(|bytes| bytes == original)
        .ok_or("jpeg range not found")?;
    let expected_revision = SourceRevision::observe(source)?;
    for _ in 0..runs {
        measure(&mut samples, "source_revision", || {
            SourceRevision::observe(source)
        })?;
        let recipe_bytes = measure(
            &mut samples,
            "known_recipe_read_two_revision_checks",
            || -> Result<Vec<u8>, Box<dyn Error>> {
                if SourceRevision::observe(source)? != expected_revision {
                    return Err("source changed".into());
                }
                let mut file = fs::File::open(source)?;
                file.seek(SeekFrom::Start(offset as u64))?;
                let mut bytes = vec![0; original.len()];
                file.read_exact(&mut bytes)?;
                if SourceRevision::observe(source)? != expected_revision {
                    return Err("source changed".into());
                }
                if seed.bytes.len() != original.len() {
                    bytes.splice(2..2, seed.bytes[2..38].iter().copied());
                }
                Ok(bytes)
            },
        )?;
        assert_eq!(recipe_bytes, seed.bytes);
        let range = measure(
            &mut samples,
            "known_source_range_read",
            || -> std::io::Result<Vec<u8>> {
                let mut file = fs::File::open(source)?;
                file.seek(SeekFrom::Start(offset as u64))?;
                let mut bytes = vec![0; original.len()];
                file.read_exact(&mut bytes)?;
                Ok(bytes)
            },
        )?;
        assert_eq!(range, original);
        measure(
            &mut samples,
            "prefix_256k_read",
            || -> std::io::Result<Vec<u8>> {
                let mut bytes = Vec::with_capacity(256 * 1024);
                fs::File::open(source)?
                    .take(256 * 1024)
                    .read_to_end(&mut bytes)?;
                Ok(bytes)
            },
        )?;
        let jpeg = measure(&mut samples, "sony_inspect_extract", || {
            sony::inspect(source, None)
        })?
        .embedded_jpeg
        .ok_or("no embedded jpeg")?;
        byte_size = jpeg.bytes.len();
        geometry = Some((jpeg.width, jpeg.height, jpeg.geometry));
        measure(&mut samples, "jpeg_decode_in_memory", || {
            image::load_from_memory(black_box(&jpeg.bytes))
        })?;

        let sync_cache = tempfile::tempdir()?;
        let result = measure(&mut samples, "sync_cold_produce_persist", || {
            preview(
                source,
                sync_cache.path(),
                RenderLevel::Thumbnail,
                PreviewPriority::Visible,
                AssetKind::Heif,
                &CancellationToken::default(),
            )
        })?;
        let warm = measure(&mut samples, "sync_warm_disk_lookup", || {
            preview(
                source,
                sync_cache.path(),
                RenderLevel::Thumbnail,
                PreviewPriority::Visible,
                AssetKind::Heif,
                &CancellationToken::default(),
            )
        })?;
        assert_eq!(result.path, warm.path);
        let bytes = measure(&mut samples, "cached_jpeg_file_read", || {
            fs::read(&warm.path)
        })?;
        assert_eq!(bytes, jpeg.bytes);

        let app_cache = tempfile::tempdir()?;
        let app = measure(&mut samples, "app_cold_publish", || {
            preview_for_app_with_completion(
                source,
                app_cache.path(),
                RenderLevel::Thumbnail,
                PreviewPriority::Visible,
                AssetKind::Heif,
                &CancellationToken::default(),
            )
        })?;
        let registry = shared_resource_registry();
        let resource_id = &app
            .result
            .resource
            .as_ref()
            .ok_or("no resource")?
            .resource_id;
        let resource = registry.resolve(resource_id).ok_or("not found")?;
        let bytes = measure(&mut samples, "app_resource_materialize", || {
            registry.materialize(&resource)
        })?;
        assert_eq!(bytes, jpeg.bytes);
        drop(resource);
        measure(
            &mut samples,
            "app_persist_remaining_wait",
            || -> Result<(), Box<dyn Error>> {
                for completion in app.completions {
                    let completed = completion.recv_timeout(std::time::Duration::from_secs(10))?;
                    if let oxy_media::PersistenceCompletion::Failed { message, .. } = completed {
                        return Err(message.into());
                    }
                }
                Ok(())
            },
        )?;
        let warm = measure(&mut samples, "app_warm_publication_lookup", || {
            preview_for_app_with_completion(
                source,
                app_cache.path(),
                RenderLevel::Thumbnail,
                PreviewPriority::Visible,
                AssetKind::Heif,
                &CancellationToken::default(),
            )
        })?;
        black_box(warm);
    }
    let output = serde_json::json!({"runs": runs, "sourceBytes": fs::metadata(source)?.len(), "thumbnailBytes": byte_size, "sourceJpegOffset": offset, "sourceJpegBytes": original.len(), "geometry": geometry, "samplesMs": samples});
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
