//! omniflux-core — the engine's root crate: the vocabulary every other crate
//! is allowed to depend on.
//!
//! Responsible for: the small, stable types and facts shared across the
//! engine — exact time, the timeline model, and the edits that change it.
//! Not responsible for: decoding, rendering, export, or anything that needs a
//! window, a codec, or a filesystem to exist.
//!
//! Module map:
//! - [`time`] — exact time: ticks and the project timebase.
//! - [`timeline`] — the model: media, tracks, clips, and the rules they obey.
//! - [`edit`] — the commands that change the model, and the history that
//!   undoes them.
//! - [`error`] — the one error type this crate returns.
//!
//! Everything here is engine vocabulary: something the UI, the CLI and the
//! codecs all have to agree on. The layout of material, the clock it is
//! measured on, and the edits that move it — nothing else.
//!
//! Note on the 4 GB target: this crate is measured on the dev machine while
//! the engine is built, and 4 GB is the goal the optimisation work aims at
//! (see the RAM census in the planning vault's Task Note). The numbers this
//! crate produces are its own — the model's memory is small and stays
//! small; the heavy part of the budget is decoding and rendering.

pub mod edit;
pub mod error;
pub mod time;
pub mod timeline;

pub use edit::{Command, History};
pub use error::CoreError;
pub use time::{Ticks, Timebase};
pub use timeline::{Clip, ClipId, MediaAsset, MediaId, Timeline, Track, TrackId, TrackKind};

/// Returns the engine's own version, sourced from the package manifest.
/// Kept as a function (not a constant) so callers can be traced in logs.
pub fn engine_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_reports_a_version() {
        let version = engine_version();

        // Deliberately NOT compared to a literal: pinning "0.1.0" here would
        // fail on every release bump while no behaviour changed. The contract
        // is "a version string callers can log", so that is what gets checked.
        // (Codebreaker Law 8: test behaviour, not implementation.)
        assert!(
            !version.is_empty(),
            "engine must always know its own version"
        );

        let major = version
            .split('.')
            .next()
            .expect("split always yields at least one part");
        assert!(
            major.parse::<u32>().is_ok(),
            "version must start with a numeric major, got {version:?}"
        );
    }
}
