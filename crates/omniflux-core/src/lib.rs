//! omniflux-core — engine fundamentals shared by everything.
//!
//! This crate is deliberately tiny right now. It exists so the workspace,
//! toolchain, and test pipeline are proven end-to-end before any real engine
//! module lands. (Codebreaker Law 0: small steps that compile.)

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
        let v = engine_version();
        assert!(!v.is_empty(), "engine must always know its own version");
        assert_eq!(v, "0.1.0");
    }
}
