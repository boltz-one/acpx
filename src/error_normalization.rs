//! Ports `others/acpx/src/acp/error-normalization.ts`.
//!
//! acpx normalizes errors into its own CLI-facing `OutputErrorCode` enum
//! (`USAGE`/`TIMEOUT`/`NO_SESSION`/`PERMISSION_DENIED`/`RUNTIME`), which is
//! explicitly out of scope for this crate (`types.rs` only ports the
//! CLI-agnostic subset of `types.ts`). What *is* in scope, and ported here,
//! is the underlying classification logic: recognizing auth-required
//! payloads, "query closed before response" disconnects, and which prompt
//! errors are safe to retry. [`normalize_agent_error`] adapts that
//! classification to produce this crate's own [`crate::AcpError`] instead.

use agent_client_protocol::Error as AcpRpcError;

use crate::error::AcpError;
use crate::error_shapes::is_acp_resource_not_found_error;

const AUTH_REQUIRED_ACP_CODE: i32 = -32000;
const QUERY_CLOSED_BEFORE_RESPONSE_DETAIL: &str = "query closed before response received";

const AUTH_REQUIRED_MESSAGE_NEEDLES: [&str; 7] = [
    "auth required",
    "authentication required",
    "authorization required",
    "credential required",
    "credentials required",
    "token required",
    "login required",
];

fn is_auth_required_message(value: &str) -> bool {
    let normalized = value.to_lowercase();
    AUTH_REQUIRED_MESSAGE_NEEDLES
        .iter()
        .any(|needle| normalized.contains(needle))
}

fn details_str<'a>(error: &'a AcpRpcError, key: &str) -> Option<&'a str> {
    error.data.as_ref()?.get(key)?.as_str()
}

/// Ports `isAcpAuthRequiredPayload`: true when the agent's JSON-RPC error
/// reports auth-required either via the dedicated ACP code, an
/// auth-flavored message, or `data.authRequired` / `data.methodId` /
/// `data.methods` hints.
pub fn is_acp_auth_required_payload(error: &AcpRpcError) -> bool {
    if i32::from(error.code) != AUTH_REQUIRED_ACP_CODE {
        return false;
    }
    if is_auth_required_message(&error.message) {
        return true;
    }
    let Some(data) = error.data.as_ref().and_then(|v| v.as_object()) else {
        return false;
    };
    data.get("authRequired").and_then(|v| v.as_bool()) == Some(true)
        || data
            .get("methodId")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty())
        || data
            .get("methods")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty())
}

/// Ports `isAcpQueryClosedBeforeResponseError`.
pub fn is_acp_query_closed_before_response_error(error: &AcpRpcError) -> bool {
    if i32::from(error.code) != -32603 {
        return false;
    }
    details_str(error, "details").is_some_and(|d| {
        d.to_lowercase()
            .contains(QUERY_CLOSED_BEFORE_RESPONSE_DETAIL)
    })
}

fn is_permanent_prompt_acp_error(error: &AcpRpcError) -> bool {
    matches!(i32::from(error.code), -32001 | -32002 | -32601 | -32602)
        || is_acp_auth_required_payload(error)
}

/// Ports `isRetryablePromptError`: true for errors from `client.prompt()`
/// that look transient (model-API 400/500s and network hiccups tend to
/// surface as ACP internal-error or parse-error codes) and thus safe to
/// retry at the prompt level.
pub fn is_retryable_prompt_error(error: &AcpRpcError) -> bool {
    if is_permanent_prompt_acp_error(error) {
        return false;
    }
    matches!(i32::from(error.code), -32603 | -32700)
}

/// Adapts acpx's `normalizeOutputError`'s classification into this crate's
/// [`AcpError`] enum, for turning a raw agent-side JSON-RPC error into a
/// typed error the rest of the crate/host application can match on.
/// `session_id` is attached to [`AcpError::SessionNotFound`] when the error
/// looks like a missing/expired session.
pub fn normalize_agent_error(error: AcpRpcError, session_id: impl Into<String>) -> AcpError {
    if is_acp_resource_not_found_error(&error) {
        return AcpError::SessionNotFound {
            session_id: session_id.into(),
        };
    }
    if is_acp_auth_required_payload(&error) {
        return AcpError::AuthPolicy(format!(
            "agent requires authentication: {} (ACP {})",
            error.message,
            i32::from(error.code)
        ));
    }
    AcpError::Other(anyhow::anyhow!(
        "ACP error {}: {}",
        i32::from(error.code),
        error.message
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_required_code_and_message_detected() {
        let error = AcpRpcError::new(-32000, "Authentication required");
        assert!(is_acp_auth_required_payload(&error));
    }

    #[test]
    fn auth_required_data_hint_detected() {
        let error =
            AcpRpcError::new(-32000, "denied").data(serde_json::json!({"methodId": "oauth"}));
        assert!(is_acp_auth_required_payload(&error));
    }

    #[test]
    fn query_closed_before_response_detected() {
        let error = AcpRpcError::new(-32603, "internal error")
            .data(serde_json::json!({"details": "Query closed before response received"}));
        assert!(is_acp_query_closed_before_response_error(&error));
    }

    #[test]
    fn retryable_vs_permanent_prompt_errors() {
        assert!(is_retryable_prompt_error(&AcpRpcError::new(
            -32603, "internal"
        )));
        assert!(is_retryable_prompt_error(&AcpRpcError::new(
            -32700,
            "parse error"
        )));
        assert!(!is_retryable_prompt_error(&AcpRpcError::new(
            -32602,
            "invalid params"
        )));
        assert!(!is_retryable_prompt_error(&AcpRpcError::new(
            -32000,
            "authentication required"
        )));
    }

    #[test]
    fn normalize_maps_not_found_to_session_not_found() {
        let error = AcpRpcError::new(-32002, "unknown session");
        let normalized = normalize_agent_error(error, "sess-1");
        assert!(matches!(
            normalized,
            AcpError::SessionNotFound { session_id } if session_id == "sess-1"
        ));
    }
}
