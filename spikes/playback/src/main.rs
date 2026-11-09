//! playback-spike CLI — thin wrapper over the library pipeline.
//! Exists so humans (and future CI) can measure real files with real numbers.
//!
//! Every run prints the ffmpeg binary it used, then the measured summary line:
//! a benchmark number without its source binary is not an honest number
//! (Law 6: measured means *stated provenance included*).

use std::path::PathBuf;
use std::process::ExitCode;

use playback_spike::{locate_ffmpeg, run_pipeline, PipelineSpec};

/// Output framing when none is passed on the command line. 720p keeps the
/// default runs cheap; 1080p budget runs pass `1920x1080` explicitly.
const DEFAULT_SCALE: (u32, u32) = (1280, 720);

fn usage() -> String {
    "usage: playback-spike <video file> [max frames] [WxH]\n  \
     WxH forces the output framing, e.g. 1280x720 (default) or 1920x1080\n  \
     optional env: OMNIFLUX_FFMPEG=<path to ffmpeg binary>"
        .to_string()
}

/// Parses `WxH` (either case of the `x`) into the pipeline's scale.
fn parse_scale(raw: &str) -> Result<(u32, u32), String> {
    let parts: Vec<&str> = raw.split(['x', 'X']).collect();
    if parts.len() != 2 {
        return Err(format!("expected WxH (e.g. 1920x1080), got {raw:?}"));
    }
    let w = parts[0]
        .parse::<u32>()
        .map_err(|_| format!("width is not a number: {}", parts[0]))?;
    let h = parts[1]
        .parse::<u32>()
        .map_err(|_| format!("height is not a number: {}", parts[1]))?;
    Ok((w, h))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!("{}", usage());
        return ExitCode::from(2);
    }
    let input = PathBuf::from(&args[0]);
    // Unparseable args fail loudly — a silently-degraded run would make the
    // printed numbers mean something else than the user believes (Law 2).
    let max_frames = match args.get(1) {
        Some(arg) => match arg.parse::<usize>() {
            Ok(n) => Some(n),
            Err(_) => {
                eprintln!("bad max frames: expected a whole number, got {arg:?}");
                return ExitCode::from(2);
            }
        },
        None => None,
    };
    let scale = match args.get(2) {
        Some(arg) => match parse_scale(arg) {
            Ok(scale) => scale,
            Err(msg) => {
                eprintln!("bad scale: {msg}");
                return ExitCode::from(2);
            }
        },
        None => DEFAULT_SCALE,
    };

    let spec = PipelineSpec {
        input,
        scale,
        max_frames,
    };

    match locate_ffmpeg() {
        Ok(ffmpeg) => match run_pipeline(&spec, &ffmpeg) {
            Ok(summary) => {
                println!("ffmpeg={}", ffmpeg.display());
                println!("{}", summary.line());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("spike failed: {error}");
                ExitCode::FAILURE
            }
        },
        Err(msg) => {
            eprintln!("spike failed: {msg}");
            ExitCode::FAILURE
        }
    }
}
