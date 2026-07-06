//! Maps `ToolCall`/`ToolCallUpdate` notifications onto
//! [`AcpRuntimeEvent::ToolCall`].

use agent_client_protocol::schema::v1::{ToolCall, ToolCallStatus, ToolCallUpdate};

use super::super::types::AcpRuntimeEvent;

pub(super) fn tool_call_text(title: &str, status: Option<&str>) -> String {
    match status {
        Some(status) if !status.is_empty() => format!("{title} ({status})"),
        _ => title.to_string(),
    }
}

pub(super) fn status_str(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Pending => "pending",
        ToolCallStatus::InProgress => "in_progress",
        ToolCallStatus::Completed => "completed",
        ToolCallStatus::Failed => "failed",
        // `#[non_exhaustive]`; see `parse_session_update`'s docs.
        _ => "pending",
    }
}

pub(super) fn tool_call_event(call: &ToolCall) -> AcpRuntimeEvent {
    let status = status_str(call.status);
    AcpRuntimeEvent::ToolCall {
        text: tool_call_text(&call.title, Some(status)),
        tag: Some("tool_call".to_string()),
        tool_call_id: Some(call.tool_call_id.0.to_string()),
        status: Some(status.to_string()),
        title: Some(call.title.clone()),
        kind: Some(call.kind),
        locations: call.locations.clone(),
        raw_input: call.raw_input.clone(),
        raw_output: call.raw_output.clone(),
        content: call.content.clone(),
    }
}

pub(super) fn tool_call_update_event(update: &ToolCallUpdate) -> AcpRuntimeEvent {
    let title = update
        .fields
        .title
        .clone()
        .unwrap_or_else(|| "tool call".to_string());
    let status = update.fields.status.map(status_str);
    AcpRuntimeEvent::ToolCall {
        text: tool_call_text(&title, status),
        tag: Some("tool_call_update".to_string()),
        tool_call_id: Some(update.tool_call_id.0.to_string()),
        status: status.map(str::to_string),
        title: update.fields.title.clone(),
        kind: update.fields.kind,
        locations: update.fields.locations.clone().unwrap_or_default(),
        raw_input: update.fields.raw_input.clone(),
        raw_output: update.fields.raw_output.clone(),
        content: update.fields.content.clone().unwrap_or_default(),
    }
}
