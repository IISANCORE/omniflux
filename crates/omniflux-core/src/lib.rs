//! omniflux-core — the engine's root crate: the vocabulary every other crate
//! is allowed to depend on.
//!
//! Responsible for: the small, stable types and facts shared across the engine.
//! Not responsible for: decoding, rendering, export, or anything that needs a
//! window, a codec, or a filesystem to exist.
//!
//! Today that vocabulary is one function (the engine's own version). The crate
//! is deliberately tiny: it exists so the workspace, toolchain, and test
//! pipeline are proven end-to-end before any real engine module lands.
//! (Codebreaker Law 0: small steps that compile.)

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
