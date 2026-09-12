//! Explicit decode and production pipeline verification; output is a fresh cache
//! directory for pipeline-auto / pipeline-camera.
//! Run each sample in a fresh process; OS file cache is not flushed.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args_os().collect::<Vec<_>>();
    if args.len() < 4
        || args[4..]
            .iter()
            .any(|arg| arg != "--decode-only" && arg != "--buffered-jpeg")
    {
        return Err("usage: raw_backend_bench <libraw|wic|wic-frame|wic-qualified|pipeline-auto|pipeline-camera> <RAW path> <output> [--decode-only] [--buffered-jpeg]".into());
    }
    match oxy_media::benchmark::raw_backend(
        std::path::Path::new(&args[2]),
        &args[1].to_string_lossy(),
        std::path::Path::new(&args[3]),
        args[4..].iter().any(|arg| arg == "--decode-only"),
        args[4..].iter().any(|arg| arg == "--buffered-jpeg"),
    ) {
        Ok(result) => println!("{result}"),
        Err(error) => {
            println!("{}", serde_json::json!({"error": error.to_string()}));
            return Err(error);
        }
    }
    Ok(())
}
