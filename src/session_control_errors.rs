//! Ports `others/acpx/src/acp/session-control-errors.ts`.
//!
//! Session-control RPCs (`session/set_mode`, `session/set_model`,
//! `session/set_config_option`) are the ones most likely to hit an adapter
//! that simply doesn't implement them. This module turns that specific
//! failure mode into a clearer message instead of a bare JSON-RPC code.

use agent_client_protocol::Error as AcpRpcError;

const SESSION_CONTROL_UNSUPPORTED_ACP_CODES: [i32; 2] = [-32601, -32602];

/// The three session-control RPCs this classification applies to. Named
/// distinctly from [`crate::jsonrpc_gap`]'s request types since
/// `session/set_model` is sent as an ACP extension method
/// (`_meta`/`ext_method`), not a first-class typed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionControlMethod {
    SetMode,
    SetModel,
    SetConfigOption,
}

impl SessionControlMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionControlMethod::SetMode => "session/set_mode",
            SessionControlMethod::SetModel => "session/set_model",
            SessionControlMethod::SetConfigOption => "session/set_config_option",
        }
    }
}

fn details_text(error: &AcpRpcError) -> Option<String> {
    let details = error.data.as_ref()?.get("details")?.as_str()?;
    let trimmed = details.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn is_likely_session_control_unsupported_error(error: &AcpRpcError) -> bool {
    let code: i32 = error.code.into();
    if SESSION_CONTROL_UNSUPPORTED_ACP_CODES.contains(&code) {
        return true;
    }
    if code != -32603 {
        return false;
    }
    details_text(error).is_some_and(|details| details.to_lowercase().contains("invalid params"))
}

/// Ports `formatSessionControlAcpSummary`.
pub fn format_session_control_acp_summary(error: &AcpRpcError) -> String {
    let code: i32 = error.code.into();
    match details_text(error) {
        Some(details) => format!(
            "{details} (ACP {code}, adapter reported \"{}\")",
            error.message
        ),
        None => format!("{} (ACP {code})", error.message),
    }
}

/// Ports `maybeWrapSessionControlError`. Returns `Some(message)` when `error`
/// looks like "the adapter doesn't implement this method / rejected this
/// value" so callers can surface a clearer message (e.g. wrapped into
/// [`crate::AcpError::SessionModeReplay`] or
/// [`crate::AcpError::SessionConfigOptionReplay`]); returns `None` when the
/// error should be propagated unchanged.
pub fn maybe_wrap_session_control_error(
    method: SessionControlMethod,
    error: &AcpRpcError,
    context: Option<&str>,
) -> Option<String> {
    if !is_likely_session_control_unsupported_error(error) {
        return None;
    }

    let summary = format_session_control_acp_summary(error);
    let context_suffix = context.map(|c| format!(" {c}")).unwrap_or_default();
    Some(format!(
        "Agent rejected {}{context_suffix}: {summary}. The adapter may not implement {}, or the requested value is not supported.",
        method.as_str(),
        method.as_str(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_not_found_is_wrapped() {
        let error = AcpRpcError::new(-32601, "method not found");
        let wrapped = maybe_wrap_session_control_error(SessionControlMethod::SetMode, &error, None);
        assert!(wrapped.unwrap().contains("session/set_mode"));
    }

    #[test]
    fn invalid_params_details_are_wrapped() {
        let error = AcpRpcError::new(-32603, "internal error")
            .data(serde_json::json!({"details": "Invalid params"}));
        let wrapped = maybe_wrap_session_control_error(
            SessionControlMethod::SetConfigOption,
            &error,
            Some("(configId=model)"),
        );
        let message = wrapped.unwrap();
        assert!(message.contains("session/set_config_option"));
        assert!(message.contains("(configId=model)"));
    }

    #[test]
    fn unrelated_error_is_not_wrapped() {
        let error = AcpRpcError::new(-32000, "auth required");
        assert!(
            maybe_wrap_session_control_error(SessionControlMethod::SetMode, &error, None).is_none()
        );
    }
}
