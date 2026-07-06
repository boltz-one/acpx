//! Public embeddable contract: the API surface a GPUI app calls. Ports
//! `others/acpx/src/runtime/public/`.

pub mod contract;
pub mod errors;
pub mod events;
pub mod handle_state;
pub mod probe;
pub mod shared;

pub use contract::{
    AcpAgentRegistry, AcpFileSessionStoreOptions, AcpRuntimeCapabilities, AcpRuntimeControl,
    AcpRuntimeDoctorReport, AcpRuntimeEnsureInput, AcpRuntimeHandle, AcpRuntimeOptions,
    AcpRuntimePromptMode, AcpRuntimeSessionMode, AcpRuntimeSessionModels, AcpRuntimeSessionUsage,
    AcpRuntimeStatus, AcpRuntimeTurn, AcpRuntimeTurnAttachment, AcpRuntimeTurnInput,
    AcpRuntimeTurnResult, AcpRuntimeTurnResultError, AcpSessionStore, BuiltInAgentRegistry,
};
pub use errors::{AcpRuntimeError, AcpRuntimeErrorCode};
pub use events::{
    AcpRuntimeAvailableCommand, AcpRuntimeEvent, AcpRuntimeTextStream, AcpRuntimeUsageBreakdown,
    AcpRuntimeUsageCost, AcpSessionUpdateTag, parse_session_update,
};
pub use handle_state::{
    decode_runtime_handle_state, encode_runtime_handle_state, write_handle_state,
};
pub use probe::{RuntimeHealthReport, probe_runtime};
pub use shared::{AcpxHandleState, derive_agent_from_session_key};

pub use crate::runtime::engine::manager::AcpRuntime;
