use oxy_media::{DiskMediaCache, MediaCache};
use sha2::{Digest, Sha256};
use std::{error::Error, fs, path::PathBuf, sync::{Arc, Barrier}, time::Instant};

fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let result = f();
    (result, start.elapsed().as_secs_f64() * 1000.0)
}

fn main() -> Result<(), Box<dyn Error>> {
    let parent = PathBuf::from(std::env::args().nth(1).ok_or("scratch parent required")?);
    fs::create_dir_all(&parent)?;
    let mut output = Vec::new();
    for count in [0, 100, 300, 625, 1100] {
        let scratch = tempfile::tempdir_in(&parent)?;
        let cache = DiskMediaCache::new(scratch.path(), 256)?;
        // Scan-only fixtures: same shard/source/file topology, no real image decode.
        for n in 0..count {
            let hash = format!("{:x}", Sha256::digest(format!("source-{n}")));
            let source = cache.root().join(&hash[..2]).join(hash);
            fs::create_dir_all(source.join(".tmp"))?;
            fs::write(source.join("thumbnail.jpg"), vec![0; 8330])?;
            fs::write(source.join("manifest.json"), b"{}")?;
        }
        let mut constructors = Vec::new();
        let mut prunes = Vec::new();
        let mut generations = Vec::new();
        for _ in 0..7 {
            let (result, elapsed) = timed(|| DiskMediaCache::new(scratch.path(), 256));
            drop(result?);
            constructors.push(elapsed);
            let (result, elapsed) = timed(|| cache.prune(u64::MAX));
            assert_eq!(result?.artifact_count, count);
            prunes.push(elapsed);
            let (result, elapsed) = timed(|| cache.generation());
            result?;
            generations.push(elapsed);
        }
        // Different cache objects, therefore different in-process mutexes, same file lock.
        let other = DiskMediaCache::new(scratch.path(), 256)?;
        let barrier = Arc::new(Barrier::new(2));
        let child_barrier = Arc::clone(&barrier);
        let child = std::thread::spawn(move || {
            child_barrier.wait();
            (0..5).map(|_| timed(|| other.prune(u64::MAX).unwrap()).1).collect::<Vec<_>>()
        });
        barrier.wait();
        let concurrent: Vec<_> = (0..50).map(|_| timed(|| cache.generation().unwrap()).1).collect();
        let concurrent_prunes = child.join().unwrap();
        output.push(serde_json::json!({"sourceCount":count,"constructorMs":constructors,
            "underBudgetPruneMs":prunes,"generationNoPruneMs":generations,
            "generationDuringPruneMs":concurrent,"concurrentPruneMs":concurrent_prunes}));
        eprintln!("completed scan/lock probe at {count} sources");
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
