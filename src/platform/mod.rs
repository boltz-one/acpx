//! Single isolation point for this crate's OS-specific branches (per
//! `docs/code-standards.md`'s platform-gate rule and ADR-0 in
//! `phase-01-crate-scaffolding.md`). Callers outside this module never see
//! `#[cfg(...)]`.

mod liveness;

// Consumed starting Phase 2 (client-process lifecycle) and Phase 3 (terminal
// manager); not yet called from this crate, hence the lint allowance.
#[allow(dead_code, unused_imports)]
pub use liveness::is_process_alive;
