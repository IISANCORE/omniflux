//! OmniFlux, prototype window.
//!
//! The engine is real; this shell exists to put it in front of a person.
//! It opens one H.264 MP4, shows the exact frame under the playhead, and
//! lets the timeline be scrubbed, split, deleted, undone and saved.
//!
//! Not here yet, by decision: audio, multiple tracks, effects, project
//! opening, file dialogs. See README.md for what the engine measures.

mod app;
mod edit_actions;
mod media;
mod preview;
mod time_text;
mod timeline_view;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let initial_media = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([880.0, 560.0])
            .with_title("OmniFlux — UI prototype"),
        ..Default::default()
    };
    eframe::run_native(
        "OmniFlux UI prototype",
        options,
        Box::new(move |cc| {
            let app = app::OmniFluxApp::new(cc, initial_media);
            Ok(Box::new(app) as Box<dyn eframe::App>)
        }),
    )
}
