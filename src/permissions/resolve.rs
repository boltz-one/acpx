//! The permission decision tree itself: policy match -> mode match ->
//! read-or-prompt fallback. Ports `resolvePermissionRequestWithDetails`/
//! `resolvePermissionRequest` from `permissions.ts`.

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionRequest, RequestPermissionResponse,
};

use super::decision::decision_to_response;
use super::escalation::build_escalation_event;
use super::policy::{
    PermissionPolicy, PermissionPolicyAction, PermissionPolicyMatch, infer_tool_kind,
    is_auto_approved_read_kind, match_permission_policy,
};
use super::responder::PermissionRequestHandler;
use super::response::{
    ResolvedPermissionRequest, cancelled, pick_option, selected, selected_or_cancelled,
    selected_or_first,
};
use crate::error::{AcpError, Result};
use crate::types::{NonInteractivePermissionPolicy, PermissionMode};

async fn resolve_interactive_prompt_result(
    params: &RequestPermissionRequest,
    handler: &dyn PermissionRequestHandler,
) -> ResolvedPermissionRequest {
    let decision = handler.request(params.clone()).await;
    ResolvedPermissionRequest {
        response: decision_to_response(params, decision),
        escalation: None,
    }
}

async fn resolve_escalating_permission_request(
    params: &RequestPermissionRequest,
    policy_match: &PermissionPolicyMatch,
    reject_option: Option<&PermissionOption>,
    handler: Option<&dyn PermissionRequestHandler>,
) -> ResolvedPermissionRequest {
    if let Some(handler) = handler {
        return resolve_interactive_prompt_result(params, handler).await;
    }

    let escalation = build_escalation_event(params, policy_match.matched_rule.clone());
    let response = reject_option
        .map(|o| selected(&o.option_id))
        .unwrap_or_else(cancelled);
    ResolvedPermissionRequest {
        response,
        escalation: Some(escalation),
    }
}

fn resolve_non_interactive_permission(
    non_interactive_policy: NonInteractivePermissionPolicy,
    reject_option: Option<&PermissionOption>,
) -> Result<ResolvedPermissionRequest> {
    if non_interactive_policy == NonInteractivePermissionPolicy::Fail {
        return Err(AcpError::PermissionPromptUnavailable);
    }
    Ok(selected_or_cancelled(reject_option))
}

async fn resolve_read_or_prompt_permission(
    params: &RequestPermissionRequest,
    non_interactive_policy: NonInteractivePermissionPolicy,
    allow_option: Option<&PermissionOption>,
    reject_option: Option<&PermissionOption>,
    handler: Option<&dyn PermissionRequestHandler>,
) -> Result<ResolvedPermissionRequest> {
    let kind = infer_tool_kind(params);
    if is_auto_approved_read_kind(kind)
        && let Some(allow_option) = allow_option
    {
        return Ok(ResolvedPermissionRequest {
            response: selected(&allow_option.option_id),
            escalation: None,
        });
    }

    let Some(handler) = handler else {
        return resolve_non_interactive_permission(non_interactive_policy, reject_option);
    };
    Ok(resolve_interactive_prompt_result(params, handler).await)
}

/// Ports `resolveModeMatch`: `approve-all`/`deny-all` short-circuit
/// regardless of policy/kind; `approve-reads` falls through to the
/// read-or-prompt fallback.
fn resolve_mode_match(
    options: &[PermissionOption],
    mode: PermissionMode,
    allow_option: Option<&PermissionOption>,
    reject_option: Option<&PermissionOption>,
) -> Option<ResolvedPermissionRequest> {
    match mode {
        PermissionMode::ApproveAll => Some(selected_or_first(options, allow_option)),
        PermissionMode::DenyAll => Some(selected_or_cancelled(reject_option)),
        PermissionMode::ApproveReads => None,
    }
}

/// Ports `resolvePermissionRequestWithDetails`: the full policy -> mode ->
/// read-or-prompt decision tree, returning the escalation audit event
/// alongside the wire response when the `escalate` policy branch fires.
pub async fn resolve_permission_request_with_details(
    params: &RequestPermissionRequest,
    mode: PermissionMode,
    non_interactive_policy: NonInteractivePermissionPolicy,
    policy: Option<&PermissionPolicy>,
    handler: Option<&dyn PermissionRequestHandler>,
) -> Result<ResolvedPermissionRequest> {
    if params.options.is_empty() {
        return Ok(ResolvedPermissionRequest {
            response: cancelled(),
            escalation: None,
        });
    }

    let allow_option = pick_option(
        &params.options,
        &[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ],
    );
    let reject_option = pick_option(
        &params.options,
        &[
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ],
    );
    let policy_match = match_permission_policy(params, policy);

    if let Some(policy_match) = &policy_match {
        let resolved = match policy_match.action {
            PermissionPolicyAction::Approve => selected_or_first(&params.options, allow_option),
            PermissionPolicyAction::Deny => selected_or_cancelled(reject_option),
            PermissionPolicyAction::Escalate => {
                resolve_escalating_permission_request(params, policy_match, reject_option, handler)
                    .await
            }
        };
        return Ok(resolved);
    }

    if let Some(resolved) = resolve_mode_match(&params.options, mode, allow_option, reject_option) {
        return Ok(resolved);
    }

    resolve_read_or_prompt_permission(
        params,
        non_interactive_policy,
        allow_option,
        reject_option,
        handler,
    )
    .await
}

/// Ports `resolvePermissionRequest`: the response-only convenience wrapper
/// around [`resolve_permission_request_with_details`].
pub async fn resolve_permission_request(
    params: &RequestPermissionRequest,
    mode: PermissionMode,
    non_interactive_policy: NonInteractivePermissionPolicy,
    policy: Option<&PermissionPolicy>,
    handler: Option<&dyn PermissionRequestHandler>,
) -> Result<RequestPermissionResponse> {
    Ok(resolve_permission_request_with_details(
        params,
        mode,
        non_interactive_policy,
        policy,
        handler,
    )
    .await?
    .response)
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
