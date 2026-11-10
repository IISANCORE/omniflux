//! census-spike CLI — what does OmniFlux actually use?
//!
//! Prints one table with the engine's real memory footprint on this machine,
//! and the verdict that goes with it. This is the baseline the 4 GB
//! optimisation will be measured against.

use std::process::ExitCode;

use census_spike::{print_census, run_census};

fn usage() -> String {
    "usage:\n  \
     census-spike [path to ffmpeg]\n\
     \n  \
     measures, on THIS machine: the editor's own process holding a 500-clip\n\
     project and 5 000 edits, the decoder at 1920x1080 and at 640x360, the\n\
     cost of one queued frame, and the project file on disk.\n\
     \n  \
     without an argument, ffmpeg is looked up the same way the playback\n\
     spike looks it up (OMNIFLUX_FFMPEG, the repo tools build, PATH)."
        .to_string()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{}", usage());
        return ExitCode::SUCCESS;
    }

    let ffmpeg = match args.first() {
        Some(path) => std::path::PathBuf::from(path),
        None => match playback_spike::locate_ffmpeg() {
            Ok(path) => path,
            Err(message) => {
                eprintln!("census failed: {message}");
                return ExitCode::FAILURE;
            }
        },
    };

    match run_census(&ffmpeg) {
        Ok(census) => {
            print_census(&census);
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("census failed: {message}");
            ExitCode::FAILURE
        }
    }
}
