// Split out of `resolve.rs` per the workspace's <200-line file guideline;
// logically still part of that module (`super::*` sees its private items).
use super::*;
use crate::permissions::responder::{ChannelPermissionRequestHandler, PermissionDecision};
use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionId, RequestPermissionOutcome, ToolCallId, ToolCallUpdate,
    ToolCallUpdateFields,
};

fn options() -> Vec<PermissionOption> {
    vec![
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
    ]
}

fn request(title: &str) -> RequestPermissionRequest {
    RequestPermissionRequest::new(
        "session-1",
        ToolCallUpdate::new(
            ToolCallId::new("tool-1"),
            ToolCallUpdateFields::new().title(title.to_string()),
        ),
        options(),
    )
}

fn class_of(response: &RequestPermissionResponse) -> &'static str {
    match &response.outcome {
        RequestPermissionOutcome::Cancelled => "cancelled",
        RequestPermissionOutcome::Selected(selected)
            if selected.option_id == PermissionOptionId::new("allow_once") =>
        {
            "allow"
        }
        RequestPermissionOutcome::Selected(_) => "reject",
        _ => "cancelled",
    }
}

#[test]
fn approve_all_mode_always_allows() {
    smol::block_on(async {
        let resolved = resolve_permission_request_with_details(
            &request("Execute: rm -rf /"),
            PermissionMode::ApproveAll,
            NonInteractivePermissionPolicy::Deny,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(class_of(&resolved.response), "allow");
    });
}

#[test]
fn deny_all_mode_always_rejects() {
    smol::block_on(async {
        let resolved = resolve_permission_request_with_details(
            &request("Read: file.txt"),
            PermissionMode::DenyAll,
            NonInteractivePermissionPolicy::Deny,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(class_of(&resolved.response), "reject");
    });
}

#[test]
fn approve_reads_auto_approves_read_kind() {
    smol::block_on(async {
        let resolved = resolve_permission_request_with_details(
            &request("Read: file.txt"),
            PermissionMode::ApproveReads,
            NonInteractivePermissionPolicy::Fail,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(class_of(&resolved.response), "allow");
    });
}

#[test]
fn approve_reads_non_read_kind_with_fail_policy_errors_without_handler() {
    smol::block_on(async {
        let result = resolve_permission_request_with_details(
            &request("Execute: rm -rf /"),
            PermissionMode::ApproveReads,
            NonInteractivePermissionPolicy::Fail,
            None,
            None,
        )
        .await;
        assert!(matches!(result, Err(AcpError::PermissionPromptUnavailable)));
    });
}

#[test]
fn approve_reads_non_read_kind_with_deny_policy_rejects_without_handler() {
    smol::block_on(async {
        let resolved = resolve_permission_request_with_details(
            &request("Execute: rm -rf /"),
            PermissionMode::ApproveReads,
            NonInteractivePermissionPolicy::Deny,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(class_of(&resolved.response), "reject");
    });
}

#[test]
fn policy_auto_deny_short_circuits_before_mode() {
    smol::block_on(async {
        let policy = PermissionPolicy {
            auto_deny: vec!["execute".to_string()],
            ..Default::default()
        };
        let resolved = resolve_permission_request_with_details(
            &request("Execute: rm -rf /"),
            PermissionMode::ApproveAll,
            NonInteractivePermissionPolicy::Deny,
            Some(&policy),
            None,
        )
        .await
        .unwrap();
        assert_eq!(class_of(&resolved.response), "reject");
    });
}

#[test]
fn escalate_without_handler_emits_escalation_and_rejects() {
    smol::block_on(async {
        let policy = PermissionPolicy {
            escalate: vec!["execute".to_string()],
            ..Default::default()
        };
        let resolved = resolve_permission_request_with_details(
            &request("Execute: rm -rf /"),
            PermissionMode::ApproveAll,
            NonInteractivePermissionPolicy::Deny,
            Some(&policy),
            None,
        )
        .await
        .unwrap();
        assert_eq!(class_of(&resolved.response), "reject");
        let escalation = resolved.escalation.expect("escalation event emitted");
        assert_eq!(escalation.matched_rule.as_deref(), Some("execute"));
    });
}

#[test]
fn escalate_with_handler_uses_decision() {
    smol::block_on(async {
        let policy = PermissionPolicy {
            escalate: vec!["execute".to_string()],
            ..Default::default()
        };
        let (handler, inbox) = ChannelPermissionRequestHandler::new();
        let consumer = smol::spawn(async move {
            let envelope = inbox.recv().await.unwrap();
            envelope.responder.respond(PermissionDecision::AllowOnce);
        });
        let resolved = resolve_permission_request_with_details(
            &request("Execute: rm -rf /"),
            PermissionMode::ApproveAll,
            NonInteractivePermissionPolicy::Deny,
            Some(&policy),
            Some(&handler),
        )
        .await
        .unwrap();
        assert_eq!(class_of(&resolved.response), "allow");
        assert!(resolved.escalation.is_none());
        consumer.await;
    });
}

#[test]
fn policy_auto_approve_matches_and_allows_independent_of_mode() {
    smol::block_on(async {
        let policy = PermissionPolicy {
            auto_approve: vec!["read".to_string()],
            ..Default::default()
        };
        // Mode is `DenyAll` — without the policy match, this request would
        // be rejected. The `Approve` policy action must win regardless.
        let resolved = resolve_permission_request_with_details(
            &request("Read: file.txt"),
            PermissionMode::DenyAll,
            NonInteractivePermissionPolicy::Deny,
            Some(&policy),
            None,
        )
        .await
        .unwrap();
        assert_eq!(class_of(&resolved.response), "allow");
        assert!(
            resolved.escalation.is_none(),
            "a plain Approve match should not emit an escalation event"
        );
    });
}

#[test]
fn policy_approve_overrides_deny_all_mode() {
    smol::block_on(async {
        let policy = PermissionPolicy {
            auto_approve: vec!["execute".to_string()],
            ..Default::default()
        };
        let resolved = resolve_permission_request_with_details(
            &request("Execute: rm -rf /"),
            PermissionMode::DenyAll,
            NonInteractivePermissionPolicy::Deny,
            Some(&policy),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            class_of(&resolved.response),
            "allow",
            "a policy Approve rule must override an otherwise-DenyAll-rejected request"
        );
    });
}

#[test]
fn policy_deny_overrides_approve_reads_mode() {
    smol::block_on(async {
        let policy = PermissionPolicy {
            auto_deny: vec!["read".to_string()],
            ..Default::default()
        };
        let resolved = resolve_permission_request_with_details(
            &request("Read: file.txt"),
            PermissionMode::ApproveReads,
            NonInteractivePermissionPolicy::Fail,
            Some(&policy),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            class_of(&resolved.response),
            "reject",
            "a policy Deny rule must override an otherwise-auto-approved read"
        );
    });
}

#[test]
fn empty_options_always_cancels() {
    smol::block_on(async {
        let request = RequestPermissionRequest::new(
            "session-1",
            ToolCallUpdate::new(ToolCallId::new("tool-1"), ToolCallUpdateFields::new()),
            vec![],
        );
        let resolved = resolve_permission_request_with_details(
            &request,
            PermissionMode::ApproveAll,
            NonInteractivePermissionPolicy::Deny,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(class_of(&resolved.response), "cancelled");
    });
}
