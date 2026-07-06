//! Maps a turn's raw outcome (ACP stop reason, timeout, or RPC error) onto
//! [`AcpRuntimeTurnResult`].

use std::time::Duration;

use agent_client_protocol::schema::v1::StopReason;

use crate::error_normalization::is_retryable_prompt_error;
use crate::runtime::public::contract::{AcpRuntimeTurnResult, AcpRuntimeTurnResultError};

pub(super) fn turn_result_from_stop_reason(stop_reason: StopReason) -> AcpRuntimeTurnResult {
    let text = match stop_reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        StopReason::Refusal => "refusal",
        StopReason::Cancelled => "cancelled",
        // `StopReason` is `#[non_exhaustive]`; treat any future variant as
        // a normal completion rather than failing to compile against a
        // schema point release.
        _ => "unknown",
    };
    if matches!(stop_reason, StopReason::Cancelled) {
        AcpRuntimeTurnResult::Cancelled {
            stop_reason: Some(text.to_string()),
        }
    } else {
        AcpRuntimeTurnResult::Completed {
            stop_reason: Some(text.to_string()),
        }
    }
}

pub(super) fn turn_result_from_timeout(timeout: Duration) -> AcpRuntimeTurnResult {
    AcpRuntimeTurnResult::Failed {
        error: AcpRuntimeTurnResultError {
            message: format!("prompt timed out after {timeout:?}"),
            code: Some("TIMEOUT".to_string()),
            detail_code: None,
            retryable: Some(true),
        },
    }
}

pub(super) fn turn_result_from_rpc_error(
    error: agent_client_protocol::Error,
) -> AcpRuntimeTurnResult {
    let retryable = is_retryable_prompt_error(&error);
    AcpRuntimeTurnResult::Failed {
        error: AcpRuntimeTurnResultError {
            message: error.message.clone(),
            code: Some(i32::from(error.code).to_string()),
            detail_code: None,
            retryable: Some(retryable),
        },
    }
}
