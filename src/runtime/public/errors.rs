//! Ports `others/acpx/src/runtime/public/errors.ts`: the runtime-level
//! error class distinct from [`crate::AcpError`] (the lower-level
//! protocol/transport error enum from Phase 1). `AcpRuntimeError` wraps a
//! coarse machine-readable code plus a human message, matching acpx's
//! `AcpRuntimeError` shape exactly (contract stability matters here since
//! the GPUI app is expected to match on `.code`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpRuntimeErrorCode {
    BackendMissing,
    BackendUnavailable,
    BackendUnsupportedControl,
    DispatchDisabled,
    InvalidRuntimeOption,
    SessionInitFailed,
    TurnFailed,
    /// Phase 6: a `start_turn` call was rejected because the session's
    /// bounded FIFO prompt queue was already at capacity (Requirement 1,
    /// `crate::queue::SessionPromptQueueError::QueueFull`). Distinct from
    /// `TurnFailed` so a caller can special-case "back off and retry" from
    /// "the turn itself failed".
    TurnQueueFull,
}

impl AcpRuntimeErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            AcpRuntimeErrorCode::BackendMissing => "ACP_BACKEND_MISSING",
            AcpRuntimeErrorCode::BackendUnavailable => "ACP_BACKEND_UNAVAILABLE",
            AcpRuntimeErrorCode::BackendUnsupportedControl => "ACP_BACKEND_UNSUPPORTED_CONTROL",
            AcpRuntimeErrorCode::DispatchDisabled => "ACP_DISPATCH_DISABLED",
            AcpRuntimeErrorCode::InvalidRuntimeOption => "ACP_INVALID_RUNTIME_OPTION",
            AcpRuntimeErrorCode::SessionInitFailed => "ACP_SESSION_INIT_FAILED",
            AcpRuntimeErrorCode::TurnFailed => "ACP_TURN_FAILED",
            AcpRuntimeErrorCode::TurnQueueFull => "ACP_TURN_QUEUE_FULL",
        }
    }
}

/// Ports `AcpRuntimeError`. Carries the lower-level [`crate::AcpError`] (if
/// any) as `source` so a caller that wants the full protocol-level detail
/// can still get it via `std::error::Error::source`.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct AcpRuntimeError {
    pub code: AcpRuntimeErrorCode,
    pub message: String,
    #[source]
    pub cause: Option<crate::AcpError>,
}

impl AcpRuntimeError {
    pub fn new(code: AcpRuntimeErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            cause: None,
        }
    }

    pub fn with_cause(
        code: AcpRuntimeErrorCode,
        message: impl Into<String>,
        cause: crate::AcpError,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            cause: Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_strings_match_acpx() {
        assert_eq!(AcpRuntimeErrorCode::TurnFailed.as_str(), "ACP_TURN_FAILED");
        assert_eq!(
            AcpRuntimeErrorCode::BackendUnsupportedControl.as_str(),
            "ACP_BACKEND_UNSUPPORTED_CONTROL"
        );
    }
}
