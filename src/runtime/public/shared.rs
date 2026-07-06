//! Ports the still-relevant half of `others/acpx/src/runtime/public/shared.ts`.
//!
//! Most of `shared.ts` is untyped-JSON-value coercion (`isRecord`,
//! `asTrimmedString`, ...) used only by acpx's raw-line event parser; this
//! port receives typed `SessionUpdate` values from the SDK instead (see
//! [`super::events`]'s module docs), so those helpers have no call site
//! here. Only [`AcpxHandleState`] and [`derive_agent_from_session_key`] are
//! ported.

use serde::{Deserialize, Serialize};

use super::contract::AcpRuntimeSessionMode;

/// Ports `AcpxHandleState`: the payload base64url-encoded into
/// [`crate::runtime::public::contract::AcpRuntimeHandle::runtime_session_name`]
/// by [`super::handle_state`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpxHandleState {
    pub name: String,
    pub agent: String,
    pub cwd: String,
    pub mode: AcpRuntimeSessionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acpx_record_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
}

/// Ports `deriveAgentFromSessionKey`: `sessionKey`s of the form
/// `agent:<name>:...` (case-insensitive prefix) name their own backend;
/// anything else falls back to `fallback_agent`.
pub fn derive_agent_from_session_key(session_key: &str, fallback_agent: &str) -> String {
    const PREFIX: &str = "agent:";
    if session_key.len() > PREFIX.len() && session_key[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
    {
        let rest = &session_key[PREFIX.len()..];
        if let Some(end) = rest.find(':') {
            let candidate = rest[..end].trim();
            if !candidate.is_empty() {
                return candidate.to_string();
            }
        }
    }
    fallback_agent.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_agent_from_prefixed_session_key() {
        assert_eq!(
            derive_agent_from_session_key("agent:claude:main", "codex"),
            "claude"
        );
    }

    #[test]
    fn falls_back_when_no_prefix_present() {
        assert_eq!(
            derive_agent_from_session_key("main-session", "codex"),
            "codex"
        );
    }

    #[test]
    fn prefix_match_is_case_insensitive() {
        assert_eq!(
            derive_agent_from_session_key("AGENT:cursor:x", "codex"),
            "cursor"
        );
    }
}
