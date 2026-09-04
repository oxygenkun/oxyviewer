//! Minimal FFmpeg HEIF GPU-path probe.
//!
//! Run from the workspace root:
//! `cargo run --release -p oxy-media --bin heif_gpu_probe -- tests/fixtures/DSC00449.HIF`

use std::{
    env,
    error::Error,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

const STACK_LAYOUT: &str = "0_0|w0_0|0_h0|w0_h0|0_h0+h2|w0_h0+h2";

struct Case {
    name: &'static str,
    args: Vec<String>,
    expected_evidence: &'static str,
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = env::args_os().nth(1).map_or_else(
        || PathBuf::from("tests/fixtures/DSC00449.HIF"),
        PathBuf::from,
    );
    #[cfg(target_os = "windows")]
    let sink = "NUL";
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let sink = "/dev/null";

    require_command("ffmpeg")?;
    println!("file={}", path.display());
    show_structure(&path)?;

    let software_filter = format!(
        "[0:0][0:1][0:2][0:3][0:4][0:5]xstack=inputs=6:layout={STACK_LAYOUT},crop=7008:4672,format=rgba[out]"
    );
    let qsv_surface_filter =
        format!("[0:0][0:1][0:2][0:3][0:4][0:5]xstack_qsv=inputs=6:layout={STACK_LAYOUT}[out]");
    let qsv_bgra_filter = format!(
        "[0:0][0:1][0:2][0:3][0:4][0:5]xstack_qsv=inputs=6:layout={STACK_LAYOUT},vpp_qsv=cw=7008:ch=4672:w=7008:h=4672:format=bgra,hwdownload,format=bgra[out]"
    );
    let common = ["-benchmark", "-hide_banner", "-loglevel", "info"];
    let path = path.to_string_lossy().into_owned();
    let cases = [
        Case {
            name: "ffmpeg software full RGBA",
            args: args(&common)
                .chain([
                    "-i",
                    &path,
                    "-filter_complex",
                    &software_filter,
                    "-map",
                    "[out]",
                ])
                .chain(["-frames:v", "1", "-f", "rawvideo", sink])
                .map(str::to_owned)
                .collect(),
            expected_evidence: "Video: rawvideo (RGBA",
        },
        Case {
            name: "Intel QSV GPU decode + xstack surface",
            args: args(&common)
                .chain([
                    "-hwaccel",
                    "qsv",
                    "-hwaccel_output_format",
                    "qsv",
                    "-i",
                    &path,
                ])
                .chain(["-filter_complex", &qsv_surface_filter, "-map", "[out]"])
                .chain(["-frames:v", "1", "-f", "null", sink])
                .map(str::to_owned)
                .collect(),
            expected_evidence: "Video: wrapped_avframe, qsv",
        },
        Case {
            name: "Intel QSV full BGRA + readback",
            args: args(&common)
                .chain([
                    "-hwaccel",
                    "qsv",
                    "-hwaccel_output_format",
                    "qsv",
                    "-i",
                    &path,
                ])
                .chain(["-filter_complex", &qsv_bgra_filter, "-map", "[out]"])
                .chain(["-frames:v", "1", "-f", "rawvideo", sink])
                .map(str::to_owned)
                .collect(),
            expected_evidence: "Video: rawvideo (BGRA",
        },
        Case {
            name: "NVIDIA CUDA tile probe",
            args: args(&common)
                .chain([
                    "-hwaccel",
                    "cuda",
                    "-hwaccel_output_format",
                    "cuda",
                    "-i",
                    &path,
                ])
                .chain(["-map", "0:0", "-frames:v", "1", "-f", "null", sink])
                .map(str::to_owned)
                .collect(),
            expected_evidence: "Hardware is lacking required capabilities",
        },
        Case {
            name: "NVIDIA D3D12 tile probe",
            args: args(&common)
                .chain([
                    "-hwaccel",
                    "d3d12va",
                    "-hwaccel_output_format",
                    "d3d12",
                    "-i",
                    &path,
                ])
                .chain(["-map", "0:0", "-frames:v", "1", "-f", "null", sink])
                .map(str::to_owned)
                .collect(),
            expected_evidence: "Video: wrapped_avframe, yuv422p10le",
        },
    ];

    for case in cases {
        run_case(case)?;
    }
    Ok(())
}

fn args<'a>(values: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    values.iter().copied()
}

fn require_command(command: &str) -> Result<(), Box<dyn Error>> {
    if Command::new(command).arg("-version").output().is_err() {
        return Err(format!("{command} is required on PATH").into());
    }
    Ok(())
}

fn show_structure(path: &PathBuf) -> Result<(), Box<dyn Error>> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v",
            "-show_entries",
            "stream=index,codec_name,profile,width,height,pix_fmt",
            "-of",
            "compact=p=0:nk=0",
        ])
        .arg(path)
        .output()?;
    println!("streams:");
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        println!("  {line}");
    }
    Ok(())
}

fn run_case(case: Case) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let output = Command::new("ffmpeg").args(&case.args).output()?;
    let elapsed = started.elapsed();
    let log = String::from_utf8_lossy(&output.stderr);
    let evidence = log.contains(case.expected_evidence);
    println!(
        "\n{}: exit={} wall={:.3}s evidence={}",
        case.name,
        output.status,
        seconds(elapsed),
        evidence
    );
    for line in log.lines().filter(|line| {
        line.contains("Using device")
            || line.contains("Video: rawvideo")
            || line.contains("Video: wrapped_avframe")
            || line.contains("Hardware is lacking")
            || line.contains("not supported with this chroma")
            || line.starts_with("bench:")
    }) {
        println!("  {line}");
    }
    Ok(())
}

fn seconds(duration: Duration) -> f64 {
    duration.as_secs_f64()
}
