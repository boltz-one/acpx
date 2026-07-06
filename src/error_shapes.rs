//! Ports `others/acpx/src/acp/error-shapes.ts`.
//!
//! acpx's version operates on `unknown` (arbitrary caught JS values) because
//! its errors can originate from anywhere (child JSON-RPC libs, user code,
//! `JSON.parse`, etc.) and it recursively hunts for an ACP-shaped
//! `{ code, message, data }` object nested under `error`/`acp`/`cause` keys.
//! This crate's agent-side errors already arrive as a typed
//! `agent_client_protocol::Error` (the JSON-RPC error object the SDK parsed
//! for us), so the "extract an ACP payload from an arbitrary value" half of
//! the TS module is unnecessary — only the classification predicates
//! (`isAcpResourceNotFoundError`'s text/code heuristics) are ported.

use agent_client_protocol::Error as AcpRpcError;
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

/// JSON-RPC codes acpx treats as "resource not found" (covers both the ACP
/// spec's dedicated code and the SDK's own not-found mapping).
pub const RESOURCE_NOT_FOUND_ACP_CODES: [i32; 2] = [-32001, -32002];

fn session_not_found_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    // Matches "session" followed by an optional quoted/unquoted id followed
    // by "not found", e.g. `Session "abc" not found`, `Session abc-123 not found`.
    PATTERN.get_or_init(|| Regex::new(r#"(?i)session\s+["'\w-]+\s+not found"#).unwrap())
}

/// Ports the TS module's private `isSessionNotFoundText`.
pub fn is_session_not_found_text(value: &str) -> bool {
    let normalized = value.to_lowercase();
    normalized.contains("resource_not_found")
        || normalized.contains("resource not found")
        || normalized.contains("session not found")
        || normalized.contains("unknown session")
        || normalized.contains("invalid session identifier")
        || session_not_found_pattern().is_match(value)
}

/// Recursively scans a JSON value (typically an error's `data` payload) for
/// a session-not-found hint, matching acpx's `hasSessionNotFoundHint`
/// depth-bounded walk (max depth 4, to avoid pathological/cyclic payloads).
pub fn has_session_not_found_hint(value: &Value, depth: u8) -> bool {
    if depth > 4 {
        return false;
    }
    match value {
        Value::String(text) => is_session_not_found_text(text),
        Value::Array(items) => items
            .iter()
            .any(|item| has_session_not_found_hint(item, depth + 1)),
        Value::Object(map) => map
            .values()
            .any(|item| has_session_not_found_hint(item, depth + 1)),
        _ => false,
    }
}

/// Ports `isAcpResourceNotFoundError`, operating on an already-typed ACP
/// JSON-RPC error instead of an arbitrary caught value.
pub fn is_acp_resource_not_found_error(error: &AcpRpcError) -> bool {
    let code: i32 = error.code.into();
    if RESOURCE_NOT_FOUND_ACP_CODES.contains(&code) {
        return true;
    }
    if is_session_not_found_text(&error.message) {
        return true;
    }
    error
        .data
        .as_ref()
        .is_some_and(|data| has_session_not_found_hint(data, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(code: i32, message: &str, data: Option<Value>) -> AcpRpcError {
        AcpRpcError::new(code, message).data(data)
    }

    #[test]
    fn resource_not_found_code_is_detected() {
        assert!(is_acp_resource_not_found_error(&err(-32001, "gone", None)));
        assert!(is_acp_resource_not_found_error(&err(-32002, "gone", None)));
    }

    #[test]
    fn session_not_found_message_text_is_detected() {
        assert!(is_acp_resource_not_found_error(&err(
            -32603,
            "Session \"abc-123\" not found",
            None
        )));
    }

    #[test]
    fn session_not_found_hint_in_nested_data_is_detected() {
        let data = serde_json::json!({"details": {"reason": "unknown session"}});
        assert!(is_acp_resource_not_found_error(&err(
            -32603,
            "internal error",
            Some(data)
        )));
    }

    #[test]
    fn unrelated_error_is_not_flagged() {
        assert!(!is_acp_resource_not_found_error(&err(
            -32602,
            "invalid params",
            None
        )));
    }
}
