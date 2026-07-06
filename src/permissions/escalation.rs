//! Audit-trail event for the `escalate` policy action. Ports
//! `PermissionEscalationEvent`/`buildEscalationEvent` from `types.ts`/
//! `permissions.ts`.

use agent_client_protocol::schema::v1::{
    RequestPermissionRequest, SessionId, ToolCallId, ToolKind,
};
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::policy::{self, infer_tool_kind};

/// Emitted whenever a policy `escalate` match couldn't be resolved by an
/// interactive handler — the audit log a user might rely on to understand
/// why an action was auto-denied pending review.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionEscalationEvent {
    #[serde(rename = "type")]
    pub event_type: &'static str,
    pub session_id: SessionId,
    pub tool_call_id: ToolCallId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub tool_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_kind: Option<ToolKind>,
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

pub(super) fn build_escalation_event(
    params: &RequestPermissionRequest,
    matched_rule: Option<String>,
) -> PermissionEscalationEvent {
    let tool_kind = infer_tool_kind(params);
    let tool_title = params
        .tool_call
        .fields
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "tool".to_string());
    PermissionEscalationEvent {
        event_type: "permission_escalation",
        session_id: params.session_id.clone(),
        tool_call_id: params.tool_call.tool_call_id.clone(),
        tool_name: policy::read_tool_name(params),
        tool_title: tool_title.clone(),
        tool_input: params.tool_call.fields.raw_input.clone(),
        tool_kind,
        action: "escalate",
        matched_rule,
        message: format!("Permission escalation required for {tool_title}"),
        timestamp: Utc::now(),
    }
}
