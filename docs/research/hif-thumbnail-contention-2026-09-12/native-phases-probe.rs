use oxy_domain::{AssetKind, PreviewPriority, RenderLevel};
use oxy_media::{preview_for_app_with_completion, shared_resource_registry};
use oxy_runtime::CancellationToken;
use std::{error::Error, fs, path::PathBuf, time::{Duration, Instant}};
fn main() -> Result<(), Box<dyn Error>> {
    let folder = PathBuf::from(std::env::args().nth(1).ok_or("source directory required")?);
    let cache = tempfile::tempdir_in(".tmp")?;
    let mut paths=fs::read_dir(folder)?.map(|e|e.map(|e|e.path())).collect::<Result<Vec<_>,_>>()?;
    paths.retain(|p|p.extension().is_some_and(|s|s.eq_ignore_ascii_case("hif")));
    paths.sort();
    let mut rows=Vec::new();
    for path in paths.iter().take(31) {
        let start=Instant::now();
        let output=preview_for_app_with_completion(path,cache.path(),RenderLevel::Thumbnail,PreviewPriority::Preload,AssetKind::Heif,&CancellationToken::default())?;
        let request_ms=start.elapsed().as_secs_f64()*1000.0;
        let registry=shared_resource_registry();
        let resource_id=&output.result.resource.as_ref().ok_or("missing resource")?.resource_id;
        let start=Instant::now();
        let resource=registry.resolve(resource_id).ok_or("missing resource")?;
        let bytes=registry.materialize(&resource)?;
        let materialize_ms=start.elapsed().as_secs_f64()*1000.0;
        assert_eq!(bytes.len(),8330);
        drop(resource);
        let start=Instant::now();
        for completion in output.completions { completion.recv_timeout(Duration::from_secs(10))?; }
        let persist_remaining_ms=start.elapsed().as_secs_f64()*1000.0;
        registry.release(resource_id);
        rows.push(serde_json::json!({"requestMs":request_ms,"materializeMs":materialize_ms,"persistenceRemainingMs":persist_remaining_ms}));
    }
    println!("{}",serde_json::to_string_pretty(&rows)?);
    Ok(())
}
