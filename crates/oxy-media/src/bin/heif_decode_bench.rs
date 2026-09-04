//! Clean full-resolution libheif decode benchmark.
//!
//! Run from the workspace root:
//! `cargo run --release -p oxy-media --bin heif_decode_bench -- tests/fixtures/DSC00449.HIF 5`

use libheif_rs::{ColorSpace, DecodingOptions, HeifContext, LibHeif, Planes, RgbChroma};
use std::{
    env,
    error::Error,
    hint::black_box,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    color_space: ColorSpace,
    threads: Option<u32>,
    convert_hdr_to_8bit: bool,
}

#[derive(Default)]
struct Timing {
    open: Duration,
    decode: Duration,
    copy: Duration,
    total: Duration,
    bytes: usize,
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = env::args_os().nth(1).map_or_else(
        || PathBuf::from("tests/fixtures/DSC00449.HIF"),
        PathBuf::from,
    );
    let repeats = env::args()
        .nth(2)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(3)
        .max(1);
    let threads = std::thread::available_parallelism()?.get() as u32;

    let context = HeifContext::read_from_file(&path.to_string_lossy())?;
    let handle = context.primary_image_handle()?;
    let libheif = LibHeif::new();
    let mut thumbnail_ids = Vec::new();
    handle.thumbnail_ids(&mut thumbnail_ids);
    println!(
        "file={} size={}x{} bit_depth={} thumbnails={:?} logical_cpus={threads} libheif={:?}",
        path.display(),
        handle.width(),
        handle.height(),
        handle.luma_bits_per_pixel(),
        thumbnail_ids,
        libheif.version(),
    );
    println!("decoders={:?}", libheif.decoder_descriptors(16, None));

    let hdr_space = if handle.luma_bits_per_pixel() > 8 {
        ColorSpace::Rgb(RgbChroma::HdrRgbLe)
    } else {
        ColorSpace::Rgb(RgbChroma::Rgb)
    };
    let cases = [
        Case {
            name: "native/default",
            color_space: ColorSpace::Undefined,
            threads: None,
            convert_hdr_to_8bit: false,
        },
        Case {
            name: "rgb8/default",
            color_space: ColorSpace::Rgb(RgbChroma::Rgb),
            threads: None,
            convert_hdr_to_8bit: true,
        },
        Case {
            name: "rgb8/1-thread",
            color_space: ColorSpace::Rgb(RgbChroma::Rgb),
            threads: Some(1),
            convert_hdr_to_8bit: true,
        },
        Case {
            name: "rgb8/all-threads",
            color_space: ColorSpace::Rgb(RgbChroma::Rgb),
            threads: Some(threads),
            convert_hdr_to_8bit: true,
        },
        Case {
            name: "source-depth/all-threads",
            color_space: hdr_space,
            threads: Some(threads),
            convert_hdr_to_8bit: false,
        },
    ];

    println!("repeats={repeats}; each iteration reopens the file");
    for case in cases {
        let mut timings = Vec::with_capacity(repeats);
        for _ in 0..repeats {
            timings.push(run_case(&libheif, &path, case)?);
        }
        print_case(case.name, &mut timings);
    }

    Ok(())
}

fn run_case(libheif: &LibHeif, path: &Path, case: Case) -> Result<Timing, Box<dyn Error>> {
    let total_started = Instant::now();
    let open_started = Instant::now();
    let context = HeifContext::read_from_file(&path.to_string_lossy())?;
    let handle = context.primary_image_handle()?;
    let open = open_started.elapsed();

    let options = DecodingOptions::new().map(|mut options| {
        if let Some(threads) = case.threads {
            options.set_num_codec_threads(threads);
            options.set_num_library_threads(threads);
        }
        if case.convert_hdr_to_8bit {
            options.set_convert_hdr_to_8bit(true);
        }
        options
    });
    let decode_started = Instant::now();
    let image = libheif.decode(&handle, case.color_space, options)?;
    let decode = decode_started.elapsed();

    let copy_started = Instant::now();
    let bytes = copy_planes(image.planes());
    let copy = copy_started.elapsed();
    black_box(bytes);

    Ok(Timing {
        open,
        decode,
        copy,
        total: total_started.elapsed(),
        bytes,
    })
}

fn copy_planes(planes: Planes<&[u8]>) -> usize {
    let mut output = Vec::new();
    for plane in [
        planes.y,
        planes.cb,
        planes.cr,
        planes.r,
        planes.g,
        planes.b,
        planes.a,
        planes.interleaved,
    ]
    .into_iter()
    .flatten()
    {
        let bytes_per_pixel = usize::from(plane.storage_bits_per_pixel).div_ceil(8);
        let row_bytes = plane.width as usize * bytes_per_pixel;
        output.reserve(row_bytes * plane.height as usize);
        for row in plane
            .data
            .chunks_exact(plane.stride)
            .take(plane.height as usize)
        {
            output.extend_from_slice(&row[..row_bytes.min(row.len())]);
        }
    }
    black_box(output).len()
}

fn print_case(name: &str, timings: &mut [Timing]) {
    timings.sort_by_key(|timing| timing.total);
    let median = &timings[timings.len() / 2];
    let minimum = &timings[0];
    println!(
        "{name:24} median total={:>8.2?} open={:>8.2?} decode={:>8.2?} copy={:>8.2?} bytes={:.1} MiB | min total={:>8.2?}",
        median.total,
        median.open,
        median.decode,
        median.copy,
        median.bytes as f64 / 1024.0 / 1024.0,
        minimum.total,
    );
}
