//! Explicit backend benchmarks, bypassing embedded previews and artifact caches.

use std::{error::Error, path::Path, time::Instant};

/// Run one full decode in a fresh process to allow external peak-memory sampling.
pub fn raw_backend(
    path: &Path,
    backend: &str,
    output: &Path,
    decode_only: bool,
    buffered_jpeg: bool,
) -> Result<serde_json::Value, Box<dyn Error>> {
    if output.exists() {
        return Err("benchmark output already exists; choose a new path".into());
    }
    let started = Instant::now();
    if matches!(backend, "pipeline-auto" | "pipeline-camera") {
        if backend != "pipeline-camera" {
            crate::request_raw_retry(path)?;
        }
        let result = crate::pipeline::raw::full_with_interim(
            path,
            output,
            false,
            &oxy_runtime::CancellationToken::default(),
        )?;
        return Ok(serde_json::json!({
            "result": result, "totalMs": started.elapsed().as_secs_f64() * 1000.0,
            "status": crate::raw_decoder_status(Some(path), false),
        }));
    }
    let (image, details) = match backend {
        "libraw" => {
            let decoded = crate::backends::libraw::developed(
                path,
                None,
                &oxy_runtime::CancellationToken::default(),
            )?;
            (decoded.image, serde_json::json!({"nativeFull": true}))
        }
        #[cfg(target_os = "windows")]
        "wic" | "wic-frame" => crate::backends::windows_wic::benchmark_raw(path, backend == "wic")?,
        #[cfg(target_os = "windows")]
        "wic-qualified" => {
            let decoded = crate::backends::windows_wic::raw::decode(
                path,
                &oxy_runtime::CancellationToken::default(),
            )?;
            (decoded.image, serde_json::to_value(decoded.facts)?)
        }
        _ => return Err(format!("unknown backend: {backend}").into()),
    };
    let decode_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let width = image.width();
    let height = image.height();
    let pixels_bytes = image.as_bytes().len();
    if decode_only {
        if output
            .extension()
            .is_some_and(|extension| extension == "png")
        {
            image.save(output)?;
        }
        return Ok(serde_json::json!({
            "backend": backend, "width": width, "height": height,
            "pixelBytes": pixels_bytes, "decodeMs": decode_ms, "details": details,
        }));
    }
    // Match the current LibRaw full presentation stage for both backends.
    let image = image.unsharpen(0.8, 2);
    let sharpen_ms = started.elapsed().as_secs_f64() * 1_000.0 - decode_ms;
    if buffered_jpeg {
        crate::cache::write_jpeg_atomically(
            &image,
            output,
            95,
            if backend == "wic-frame" {
                crate::presentation::CAMERA_JPEG
            } else {
                crate::presentation::RAW_DEVELOPED_JPEG
            },
        )?;
    } else {
        encode_unbuffered_baseline(&image, output, backend != "wic-frame")?;
    }
    let total_ms = started.elapsed().as_secs_f64() * 1_000.0;
    Ok(serde_json::json!({
        "backend": backend,
        "width": width,
        "height": height,
        "pixelBytes": pixels_bytes,
        "decodeMs": decode_ms,
        "sharpenMs": sharpen_ms,
        "encodeMs": total_ms - decode_ms - sharpen_ms,
        "totalMs": total_ms,
        "jpegBytes": std::fs::metadata(output)?.len(),
        "details": details,
        "jpegWriter": if buffered_jpeg { "buffered-production" } else { "unbuffered-baseline" },
    }))
}

fn encode_unbuffered_baseline(
    image: &image::DynamicImage,
    output: &Path,
    srgb: bool,
) -> Result<(), Box<dyn Error>> {
    use image::{ImageEncoder, codecs::jpeg::JpegEncoder};
    let mut temporary = tempfile::NamedTempFile::new_in(output.parent().unwrap_or(Path::new(".")))?;
    {
        let mut encoder = JpegEncoder::new_with_quality(&mut temporary, 95);
        if srgb {
            encoder.set_icc_profile(lcms2::Profile::new_srgb().icc()?)?;
        }
        encoder.encode_image(image)?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(output)?;
    Ok(())
}
