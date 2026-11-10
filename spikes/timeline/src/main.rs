//! timeline-spike CLI — the timeline model's edit/undo machinery, measured.
//!
//! Prints the numbers for this run and the verdict they support. A
//! measurement without its inputs is a rumour, so the project size and the
//! edit count are printed with every result.

use std::process::ExitCode;

use timeline_spike::{print_report, run_spike, SpikeConfig};

fn usage() -> String {
    "usage:\n  \
     timeline-spike [clips] [edits]\n\
     \n  \
     builds a project of <clips> clips, makes <edits> edits through the\n\
     command history, then undoes and redoes everything, printing the\n\
     measured cost of an edit, of an undo, and of one undoable edit in RAM.\n\
     \n  \
     defaults: 500 clips, 5000 edits."
        .to_string()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{}", usage());
        return ExitCode::SUCCESS;
    }

    let mut config = SpikeConfig::default();
    if let Some(arg) = args.first() {
        match arg.parse::<usize>() {
            Ok(clips) => config.clips = clips,
            Err(_) => {
                eprintln!("bad clip count: expected a whole number, got {arg:?}");
                return ExitCode::from(2);
            }
        }
    }
    if let Some(arg) = args.get(1) {
        match arg.parse::<usize>() {
            Ok(edits) => config.edits = edits,
            Err(_) => {
                eprintln!("bad edit count: expected a whole number, got {arg:?}");
                return ExitCode::from(2);
            }
        }
    }

    match run_spike(&config) {
        Ok(report) => {
            print_report(&report);
            // The promise the whole design rests on: undoing everything puts
            // the project back. A spike that quietly fails its own promise
            // must not print a happy summary (Law 3).
            if !report.undo_restored_project || !report.redo_restored_project {
                eprintln!(
                    "timeline spike failed: undoing every edit did not restore the project \
                     (undo_restored={}, redo_restored={})",
                    report.undo_restored_project, report.redo_restored_project
                );
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("timeline spike failed: {message}");
            ExitCode::FAILURE
        }
    }
}
