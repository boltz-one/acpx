use super::*;
use agent_client_protocol::schema::v1::{ToolCallId, ToolCallUpdate, ToolCallUpdateFields};

fn request_with_title(title: &str) -> RequestPermissionRequest {
    RequestPermissionRequest::new(
        "session-1",
        ToolCallUpdate::new(
            ToolCallId::new("tool-1"),
            ToolCallUpdateFields::new().title(title.to_string()),
        ),
        vec![],
    )
}

#[test]
fn infers_read_kind_from_title() {
    let params = request_with_title("Read: src/main.rs");
    assert_eq!(infer_tool_kind(&params), Some(ToolKind::Read));
}

#[test]
fn infers_other_when_title_head_unmatched() {
    // "frobnicate" happens to contain "cat" as a substring, which would
    // (faithfully, matching acpx's own `head.includes(needle)` check) infer
    // `Read` — use a head with no needle substrings at all to test the true
    // "no match" fallback.
    let params = request_with_title("Zzyzx: widget");
    assert_eq!(infer_tool_kind(&params), Some(ToolKind::Other));
}

#[test]
fn no_title_infers_no_kind() {
    let params = RequestPermissionRequest::new(
        "session-1",
        ToolCallUpdate::new(ToolCallId::new("tool-1"), ToolCallUpdateFields::new()),
        vec![],
    );
    assert_eq!(infer_tool_kind(&params), None);
}

#[test]
fn wildcard_rule_matches_anything() {
    let params = request_with_title("Execute: rm -rf");
    let policy = PermissionPolicy {
        auto_deny: vec!["*".to_string()],
        ..Default::default()
    };
    let result = match_permission_policy(&params, Some(&policy)).unwrap();
    assert_eq!(result.action, PermissionPolicyAction::Deny);
    assert_eq!(result.matched_rule.as_deref(), Some("*"));
}

#[test]
fn matches_rule_by_inferred_kind_token() {
    let params = request_with_title("Read: src/main.rs");
    let policy = PermissionPolicy {
        auto_approve: vec!["read".to_string()],
        ..Default::default()
    };
    let result = match_permission_policy(&params, Some(&policy)).unwrap();
    assert_eq!(result.action, PermissionPolicyAction::Approve);
}

#[test]
fn no_policy_never_matches() {
    let params = request_with_title("Read: src/main.rs");
    assert!(match_permission_policy(&params, None).is_none());
}

#[test]
fn default_action_used_when_no_rule_matches() {
    let params = request_with_title("Read: src/main.rs");
    let policy = PermissionPolicy {
        default_action: Some(PermissionPolicyAction::Escalate),
        ..Default::default()
    };
    let result = match_permission_policy(&params, Some(&policy)).unwrap();
    assert_eq!(result.action, PermissionPolicyAction::Escalate);
    assert_eq!(result.matched_rule, None);
}
