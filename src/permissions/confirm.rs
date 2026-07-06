//! Shared allow/reject confirmation gate used by both `filesystem.rs`
//! (`isWriteApproved`/read `deny-all` check) and `terminal::TerminalManager`
//! (`isExecuteApproved`) in acpx. Both call sites funnel through the same
//! `promptForPermission` stdin prompt in acpx; here they funnel through the
//! same [`PermissionRequestHandler`] (ADR-6) via a synthetic
//! allow-once/reject-once [`RequestPermissionRequest`], keeping one
//! interactive-decision mechanism for the whole crate instead of three
//! divergent callback shapes.

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionId, PermissionOptionKind, RequestPermissionRequest,
    SessionId, ToolCallId, ToolCallUpdate, ToolCallUpdateFields,
};

use super::responder::{PermissionDecision, PermissionRequestHandler};
use crate::error::{AcpError, Result};
use crate::types::{NonInteractivePermissionPolicy, PermissionMode};

/// Builds a minimal, allow-once/reject-once permission request describing a
/// single yes/no confirmation (a filesystem write or a terminal command),
/// distinct from the richer tool-call permission requests an agent sends
/// over `session/request_permission`.
fn confirm_request(session_id: SessionId, title: String) -> RequestPermissionRequest {
    let options = vec![
        PermissionOption::new(
            PermissionOptionId::new("allow_once"),
            "Allow",
            PermissionOptionKind::AllowOnce,
        ),
        PermissionOption::new(
            PermissionOptionId::new("reject_once"),
            "Reject",
            PermissionOptionKind::RejectOnce,
        ),
    ];
    RequestPermissionRequest::new(
        session_id,
        ToolCallUpdate::new(
            ToolCallId::new(uuid::Uuid::new_v4().to_string()),
            ToolCallUpdateFields::new().title(title),
        ),
        options,
    )
}

/// Ports the shared shape of acpx's `isWriteApproved`/`isExecuteApproved`:
/// `approve-all` always allows, `deny-all` always denies, otherwise defer to
/// the injected [`PermissionRequestHandler`] (or the non-interactive
/// fallback policy when no handler is configured).
pub async fn confirm_action(
    mode: PermissionMode,
    non_interactive_policy: NonInteractivePermissionPolicy,
    handler: Option<&dyn PermissionRequestHandler>,
    session_id: SessionId,
    title: String,
) -> Result<bool> {
    match mode {
        PermissionMode::ApproveAll => Ok(true),
        PermissionMode::DenyAll => Ok(false),
        PermissionMode::ApproveReads => {
            let Some(handler) = handler else {
                return match non_interactive_policy {
                    NonInteractivePermissionPolicy::Fail => {
                        Err(AcpError::PermissionPromptUnavailable)
                    }
                    NonInteractivePermissionPolicy::Deny => Ok(false),
                };
            };
            let decision = handler.request(confirm_request(session_id, title)).await;
            Ok(matches!(
                decision,
                PermissionDecision::AllowOnce | PermissionDecision::AllowAlways
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::ChannelPermissionRequestHandler;

    #[test]
    fn approve_all_never_prompts() {
        smol::block_on(async {
            let approved = confirm_action(
                PermissionMode::ApproveAll,
                NonInteractivePermissionPolicy::Fail,
                None,
                SessionId::new("s1"),
                "write to file".to_string(),
            )
            .await
            .unwrap();
            assert!(approved);
        });
    }

    #[test]
    fn deny_all_never_prompts() {
        smol::block_on(async {
            let approved = confirm_action(
                PermissionMode::DenyAll,
                NonInteractivePermissionPolicy::Fail,
                None,
                SessionId::new("s1"),
                "write to file".to_string(),
            )
            .await
            .unwrap();
            assert!(!approved);
        });
    }

    #[test]
    fn approve_reads_without_handler_and_fail_policy_errors() {
        smol::block_on(async {
            let result = confirm_action(
                PermissionMode::ApproveReads,
                NonInteractivePermissionPolicy::Fail,
                None,
                SessionId::new("s1"),
                "write to file".to_string(),
            )
            .await;
            assert!(matches!(result, Err(AcpError::PermissionPromptUnavailable)));
        });
    }

    #[test]
    fn approve_reads_with_handler_uses_decision() {
        smol::block_on(async {
            let (handler, inbox) = ChannelPermissionRequestHandler::new();
            let consumer = smol::spawn(async move {
                let envelope = inbox.recv().await.unwrap();
                envelope.responder.respond(PermissionDecision::AllowAlways);
            });
            let approved = confirm_action(
                PermissionMode::ApproveReads,
                NonInteractivePermissionPolicy::Deny,
                Some(&handler),
                SessionId::new("s1"),
                "run command".to_string(),
            )
            .await
            .unwrap();
            assert!(approved);
            consumer.await;
        });
    }
}
