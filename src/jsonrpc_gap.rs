//! ADR-1 gap-methods module: `session/set_mode`, `session/set_config_option`,
//! `terminal/release`.
//!
//! **Deviation from the phase's original ADR-1 text, discovered while
//! implementing:** ADR-1 was written assuming (per researcher-01) that the
//! pinned `agent-client-protocol` Rust SDK does not expose these three
//! methods as typed request/response structs, and planned to hand-roll them
//! here. Inspecting the actual vendored SDK source
//! (`agent-client-protocol` 1.0.1 / `agent-client-protocol-schema` 1.1.0,
//! the exact versions this workspace pins) shows all three are already
//! fully typed via the SDK's `impl_jsonrpc_request!` machinery:
//! - `SetSessionModeRequest`/`Response` and
//!   `SetSessionConfigOptionRequest`/`Response` in
//!   `schema::v1::client_to_agent` (client -> agent requests — we send
//!   these).
//! - `ReleaseTerminalRequest`/`Response` in `schema::v1::agent_to_client`
//!   (agent -> client requests — the agent sends these to *us*; Phase 3's
//!   terminal manager registers the handler for it via
//!   `Builder::on_receive_request::<ReleaseTerminalRequest>`, not this
//!   module).
//!
//! Hand-rolling parallel structs here would duplicate the SDK's own typed
//! definitions (DRY violation) and risk drifting from its wire format, so
//! this module is a thin, documented pass-through instead of the originally
//! planned hand-rolled implementation. The "raw-request escape hatch"
//! contingency ADR-1 described is unnecessary: `ConnectionTo::send_request`
//! already accepts any `JsonRpcRequest` type, typed or not.

use agent_client_protocol::schema::v1::{
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse,
};
use agent_client_protocol::{Agent, ConnectionTo, Error as AcpRpcError};

// Re-exported so Phase 3's terminal manager can reference the agent-initiated
// counterpart from a single well-known path alongside the client-initiated
// two below, without reaching into `agent_client_protocol` schema modules
// directly.
pub use agent_client_protocol::schema::v1::{ReleaseTerminalRequest, ReleaseTerminalResponse};

/// Sends `session/set_mode` and awaits the typed response.
pub async fn send_set_session_mode(
    cx: &ConnectionTo<Agent>,
    request: SetSessionModeRequest,
) -> Result<SetSessionModeResponse, AcpRpcError> {
    cx.send_request(request).block_task().await
}

/// Sends `session/set_config_option` and awaits the typed response.
pub async fn send_set_session_config_option(
    cx: &ConnectionTo<Agent>,
    request: SetSessionConfigOptionRequest,
) -> Result<SetSessionConfigOptionResponse, AcpRpcError> {
    cx.send_request(request).block_task().await
}
