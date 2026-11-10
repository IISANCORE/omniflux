//! playback-spike CLI — thin wrapper over the library pipeline.
//! Exists so humans (and future CI) can measure real files with real numbers.
//!
//! Every run prints the ffmpeg binary it used, then the measured summary line:
//! a benchmark number without its source binary is not an honest number
//! (Law 6: measured means *stated provenance included*).

use std::path::PathBuf;
use std::process::ExitCode;

use playback_spike::{locate_ffmpeg, run_pipeline, warm, PipelineSpec};

/// Output framing when none is passed on the command line. 720p keeps the
/// default runs cheap; 1080p budget runs pass `1920x1080` explicitly.
const DEFAULT_SCALE: (u32, u32) = (1280, 720);

/// Keyframe jumps per seek run when none is passed on the command line.
const DEFAULT_SEEK_JUMPS: usize = 8;

fn usage() -> String {
    "usage:\n  \
     playback-spike <video file> [max frames] [WxH]\n  \
     playback-spike seek <video file> [jumps]\n\
     \n  \
     decode mode: WxH forces the output framing (default 1280x720)\n  \
     seek mode: measures warm (one persistent decoder) vs cold (spawn per\n  \
     jump) time-to-frame on keyframe jumps, default 8 jumps\n\
     \n  \
     optional env: OMNIFLUX_FFMPEG=<path to ffmpeg binary>\n  \
     optional env: OMNIFLUX_FFPROBE=<path to ffprobe binary> (seek mode; defaults\n  \
     to the ffprobe sitting next to the located ffmpeg)"
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

    if args[0] == "seek" {
        let input = match args.get(1) {
            Some(f) => PathBuf::from(f),
            None => {
                eprintln!("seek mode needs a file: playback-spike seek <file> [jumps]");
                return ExitCode::from(2);
            }
        };
        let jumps = match args.get(2) {
            Some(arg) => match arg.parse::<usize>() {
                Ok(n) if n >= 1 => n,
                Ok(_) => {
                    eprintln!("bad jumps: expected at least 1, got {arg:?}");
                    return ExitCode::from(2);
                }
                Err(_) => {
                    eprintln!("bad jumps: expected a whole number, got {arg:?}");
                    return ExitCode::from(2);
                }
            },
            None => DEFAULT_SEEK_JUMPS,
        };
        return match locate_ffmpeg() {
            Ok(ffmpeg) => {
                // Provenance first: a benchmark without its source binary is
                // not an honest number (Law 6).
                println!("ffmpeg={}", ffmpeg.display());
                match warm::run_seek_spike(&ffmpeg, &input, jumps) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(msg) => {
                        eprintln!("seek spike failed: {msg}");
                        ExitCode::FAILURE
                    }
                }
            }
            Err(msg) => {
                eprintln!("seek spike failed: {msg}");
                ExitCode::FAILURE
            }
        };
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
