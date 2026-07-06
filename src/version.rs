//! Crate version reporting.
//!
//! Adapts (does not port) `others/acpx/src/version.ts`: acpx resolves its
//! version from `package.json` at runtime (walking up from the CLI's install
//! location) because it ships as an npm package with no fixed build-time
//! version constant. This crate is compiled, so `env!("CARGO_PKG_VERSION")`
//! is the direct Rust equivalent — there is no analogous "found the wrong
//! nested package.json" failure mode to guard against.

/// The `boltz-acpx` crate version, e.g. `"0.1.0"`.
pub const ACP_CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns [`ACP_CRATE_VERSION`]. Kept as a function (rather than requiring
/// callers to reference the const directly) so call sites read the same as
/// acpx's `getAcpxVersion()`, and so the resolution strategy can grow
/// (e.g. an app-provided override) without changing call sites.
pub fn crate_version() -> &'static str {
    ACP_CRATE_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_version_is_non_empty() {
        assert!(!crate_version().is_empty());
    }
}
