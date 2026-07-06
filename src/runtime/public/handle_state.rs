//! Ports `others/acpx/src/runtime/public/handle-state.ts`: encodes the
//! bits of state a caller needs to resume/identify a session
//! (`agent`/`cwd`/`mode`/backend ids) into
//! [`AcpRuntimeHandle::runtime_session_name`] as an opaque, versioned
//! string.
//!
//! acpx base64url-encodes the JSON payload (Node's `Buffer` makes that the
//! path of least resistance there); this port percent-encodes it instead
//! (via the `percent-encoding` crate, already a dependency for
//! [`crate::session::store_options::safe_session_id`]) to avoid adding a
//! new dependency for what is, on both sides, just "an opaque string a
//! human should not hand-edit" — no external tool decodes this format.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

use super::contract::AcpRuntimeHandle;
use super::shared::AcpxHandleState;

const RUNTIME_HANDLE_PREFIX: &str = "acpx:v2:";

const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Ports `encodeAcpxRuntimeHandleState`.
pub fn encode_runtime_handle_state(state: &AcpxHandleState) -> String {
    let json = serde_json::to_string(state).unwrap_or_default();
    format!(
        "{RUNTIME_HANDLE_PREFIX}{}",
        utf8_percent_encode(&json, COMPONENT)
    )
}

/// Ports `decodeAcpxRuntimeHandleState`.
pub fn decode_runtime_handle_state(runtime_session_name: &str) -> Option<AcpxHandleState> {
    let trimmed = runtime_session_name.trim();
    let payload = trimmed.strip_prefix(RUNTIME_HANDLE_PREFIX)?;
    let raw = percent_decode_str(payload).decode_utf8().ok()?;
    serde_json::from_str(&raw).ok()
}

/// Ports `writeHandleState`.
pub fn write_handle_state(handle: &mut AcpRuntimeHandle, state: AcpxHandleState) {
    handle.cwd = Some(state.cwd.clone());
    handle.acpx_record_id = state.acpx_record_id.clone();
    handle.backend_session_id = state.backend_session_id.clone();
    handle.agent_session_id = state.agent_session_id.clone();
    handle.runtime_session_name = encode_runtime_handle_state(&state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::public::contract::AcpRuntimeSessionMode;

    fn sample_state() -> AcpxHandleState {
        AcpxHandleState {
            name: "main".into(),
            agent: "claude".into(),
            cwd: "/tmp/project".into(),
            mode: AcpRuntimeSessionMode::Persistent,
            acpx_record_id: Some("rec-1".into()),
            backend_session_id: Some("sess-1".into()),
            agent_session_id: None,
        }
    }

    #[test]
    fn round_trips_through_encode_decode() {
        let state = sample_state();
        let encoded = encode_runtime_handle_state(&state);
        assert!(encoded.starts_with("acpx:v2:"));
        let decoded = decode_runtime_handle_state(&encoded).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn decode_rejects_missing_prefix() {
        assert!(decode_runtime_handle_state("not-a-handle").is_none());
    }

    #[test]
    fn write_handle_state_populates_identity_fields() {
        let mut handle = AcpRuntimeHandle {
            session_key: "key".into(),
            backend: "claude".into(),
            runtime_session_name: String::new(),
            cwd: None,
            acpx_record_id: None,
            backend_session_id: None,
            agent_session_id: None,
        };
        write_handle_state(&mut handle, sample_state());
        assert_eq!(handle.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(handle.backend_session_id.as_deref(), Some("sess-1"));
        assert!(handle.runtime_session_name.starts_with("acpx:v2:"));
    }
}
