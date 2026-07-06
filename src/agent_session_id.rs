//! Ports `others/acpx/src/acp/agent-session-id.ts`.
//!
//! Some ACP agents echo their own provider-side session id back through a
//! response's `_meta` object under one of a small set of known keys. This is
//! used to persist/replay that id across process restarts (Phase 5).

use serde_json::{Map, Value};

/// `_meta` keys agents have been observed to use for their own session id,
/// checked in order. Mirrors acpx's `AGENT_SESSION_ID_META_KEYS`.
pub const AGENT_SESSION_ID_META_KEYS: [&str; 2] = ["agentSessionId", "sessionId"];

/// Trims `value` and returns `None` if it is empty. Ports
/// `normalizeAgentSessionId`.
pub fn normalize_agent_session_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Looks up [`AGENT_SESSION_ID_META_KEYS`] in `meta`, in order, returning the
/// first non-empty string value found. Ports `extractAgentSessionId`.
pub fn extract_agent_session_id(meta: Option<&Map<String, Value>>) -> Option<String> {
    let meta = meta?;
    AGENT_SESSION_ID_META_KEYS.iter().find_map(|key| {
        meta.get(*key)
            .and_then(Value::as_str)
            .and_then(normalize_agent_session_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_first_matching_key() {
        let meta = json!({"agentSessionId": " abc "}).as_object().cloned();
        assert_eq!(
            extract_agent_session_id(meta.as_ref()),
            Some("abc".to_string())
        );
    }

    #[test]
    fn falls_back_to_second_key() {
        let meta = json!({"sessionId": "xyz"}).as_object().cloned();
        assert_eq!(
            extract_agent_session_id(meta.as_ref()),
            Some("xyz".to_string())
        );
    }

    #[test]
    fn returns_none_when_absent_or_blank() {
        let meta = json!({"agentSessionId": "   "}).as_object().cloned();
        assert_eq!(extract_agent_session_id(meta.as_ref()), None);
        assert_eq!(extract_agent_session_id(None), None);
    }
}
