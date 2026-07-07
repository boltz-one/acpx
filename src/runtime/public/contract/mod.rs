//! The public embeddable contract. Ports
//! `others/acpx/src/runtime/public/contract.ts` — the actual API surface the
//! GPUI app calls. Per the phase brief, shape fidelity to this file matters
//! more than internal implementation fidelity elsewhere in `runtime/`.
//!
//! ## ADR-7 recap (ensureSession/startTurn shape)
//!
//! acpx's `AcpRuntime` is a TS *interface* implemented by exactly one
//! concrete runtime; nothing in this crate's scope needs genuine
//! substitutability (no test double stands in for the runtime — Success
//! Criteria call for exercising the *real* fake-agent subprocess). This
//! port is therefore a concrete `AcpRuntime` struct, not a trait, per
//! Requirement 1's decision guidance. `AcpSessionStore`/`AcpAgentRegistry`
//! *are* traits (contract.ts declares them as such, and a GPUI app
//! plausibly wants to substitute its own storage/registry), and
//! `PermissionRequestHandler` is reused as-is from Phase 3 (ADR-6 already
//! solved "async, non-blocking decision callback" — redefining an
//! equivalent callback type here would duplicate it).
//!
//! Split (per the workspace's <200-line file guideline) along contract.ts's
//! own type clusters: [`types`] (plain data shapes), [`turn`] (the one type
//! with real behavior), [`registry`] (the store/registry traits), and
//! [`options`] (construction-time config plus the two functions that bridge
//! these types to the rest of `runtime::public`). All items stay reachable
//! at this module's path via the re-exports below, matching acpx's single
//! `contract.ts` surface.

mod options;
mod registry;
mod turn;
mod types;

pub use options::{AcpFileSessionStoreOptions, AcpRuntimeOptions, OnFsWriteHook};
pub(crate) use options::{attachment_content_blocks, legacy_terminal_event_from_turn_result};
pub use registry::{AcpAgentRegistry, AcpSessionStore, BuiltInAgentRegistry};
pub use turn::AcpRuntimeTurn;
pub use types::{
    AcpRuntimeCapabilities, AcpRuntimeControl, AcpRuntimeDoctorReport, AcpRuntimeEnsureInput,
    AcpRuntimeHandle, AcpRuntimePromptMode, AcpRuntimeSessionMode, AcpRuntimeSessionModels,
    AcpRuntimeSessionUsage, AcpRuntimeStatus, AcpRuntimeTurnAttachment, AcpRuntimeTurnInput,
    AcpRuntimeTurnResult, AcpRuntimeTurnResultError,
};
