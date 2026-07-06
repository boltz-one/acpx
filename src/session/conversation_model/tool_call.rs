//! Tool-call-update application.
//!
//! Ports `applyToolCallUpdate` from
//! `others/acpx/src/session/conversation-model.ts`. See
//! [`super::session_update`]'s module docs for why [`ToolCallUpdateInput`]
//! isn't the real ACP `ToolCall`/`ToolCallUpdate` type. `ToolUse`
//! lookup/creation and tool-result upsert live in [`super::tool_use`]
//! (split out to stay under this crate's per-file line convention).

use serde_json::Value;

use super::message::SessionAgentMessage;
use super::tool_use::{
    ensure_tool_use_content, to_raw_input, to_tool_result_content, upsert_tool_result,
};

/// Ports the `ToolCall | ToolCallUpdate` union acpx's `applyToolCallUpdate`
/// accepts, flattened to the fields it actually reads. `None` means "field
/// absent from this update" (acpx's `hasOwn(update, key)` checks), distinct
/// from a present-but-empty value.
#[derive(Debug, Clone, Default)]
pub struct ToolCallUpdateInput {
    pub tool_call_id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub raw_input: Option<Value>,
    pub raw_output_present: bool,
    pub raw_output: Option<Value>,
}

fn status_indicates_complete(status: &str) -> bool {
    let normalized = status.to_lowercase();
    ["complete", "done", "success", "failed", "error", "cancel"]
        .iter()
        .any(|needle| normalized.contains(needle))
}

/// Ports `statusIndicatesError`: always a concrete `bool`, `false` when
/// `status` is absent (ADR-9, gap 19) — the caller no longer distinguishes
/// "no opinion, preserve prior value" from "explicitly not an error".
fn status_indicates_error(status: Option<&str>) -> bool {
    let Some(status) = status else {
        return false;
    };
    let normalized = status.to_lowercase();
    normalized.contains("fail") || normalized.contains("error")
}

/// Ports `applyToolCallUpdate`.
pub fn apply_tool_call_update(agent: &mut SessionAgentMessage, update: &ToolCallUpdateInput) {
    let tool = ensure_tool_use_content(agent, &update.tool_call_id);

    if let Some(title) = &update.title {
        let normalized = title.trim();
        if !normalized.is_empty() {
            tool.name = normalized.to_string();
        }
    }
    if let Some(kind) = &update.kind {
        let normalized = kind.trim();
        if (tool.name.is_empty() || tool.name == "tool_call") && !normalized.is_empty() {
            tool.name = normalized.to_string();
        }
    }
    if let Some(raw_input) = &update.raw_input {
        tool.input = raw_input.clone();
        tool.raw_input = to_raw_input(Some(raw_input));
    }
    if let Some(status) = &update.status {
        tool.is_input_complete = status_indicates_complete(status);
    }

    let has_result_patch = update.raw_output_present
        || update.status.is_some()
        || update.title.is_some()
        || update.kind.is_some();
    if has_result_patch {
        let tool_name = tool.name.clone();
        let is_error = status_indicates_error(update.status.as_deref());
        let content = update
            .raw_output
            .as_ref()
            .map(|value| to_tool_result_content(Some(value)));
        upsert_tool_result(
            agent,
            &update.tool_call_id,
            Some(tool_name),
            is_error,
            content,
            update.raw_output.clone(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::conversation_model::message::SessionAgentContent;

    #[test]
    fn apply_tool_call_update_creates_and_updates_tool_use() {
        let mut agent = SessionAgentMessage::default();
        apply_tool_call_update(
            &mut agent,
            &ToolCallUpdateInput {
                tool_call_id: "t1".into(),
                title: Some("Read File".into()),
                raw_input: Some(serde_json::json!({"path": "a.txt"})),
                status: Some("in_progress".into()),
                ..Default::default()
            },
        );

        let SessionAgentContent::ToolUse(tool) = &agent.content[0] else {
            panic!("expected ToolUse content");
        };
        assert_eq!(tool.name, "Read File");
        assert!(!tool.is_input_complete);

        apply_tool_call_update(
            &mut agent,
            &ToolCallUpdateInput {
                tool_call_id: "t1".into(),
                status: Some("completed".into()),
                raw_output_present: true,
                raw_output: Some(serde_json::json!("done")),
                ..Default::default()
            },
        );

        let result = agent.tool_results.get("t1").unwrap();
        assert!(!result.is_error);
        assert_eq!(result.tool_name, "Read File");
    }

    #[test]
    fn apply_tool_call_update_marks_error_status() {
        let mut agent = SessionAgentMessage::default();
        apply_tool_call_update(
            &mut agent,
            &ToolCallUpdateInput {
                tool_call_id: "t1".into(),
                status: Some("failed".into()),
                raw_output_present: true,
                raw_output: Some(serde_json::json!("boom")),
                ..Default::default()
            },
        );
        assert!(agent.tool_results.get("t1").unwrap().is_error);
    }

    /// ADR-9 (gap 19): a prior error-flagged update must not stay "sticky"
    /// across a later result-triggering update that carries no `status` —
    /// `is_error` resets to `false`, matching acpx's always-concrete-bool
    /// `statusIndicatesError` exactly.
    #[test]
    fn apply_tool_call_update_resets_is_error_when_status_absent() {
        let mut agent = SessionAgentMessage::default();
        apply_tool_call_update(
            &mut agent,
            &ToolCallUpdateInput {
                tool_call_id: "t1".into(),
                status: Some("failed".into()),
                raw_output_present: true,
                raw_output: Some(serde_json::json!("boom")),
                ..Default::default()
            },
        );
        assert!(agent.tool_results.get("t1").unwrap().is_error);

        // Title-only update: no `status` field present at all.
        apply_tool_call_update(
            &mut agent,
            &ToolCallUpdateInput {
                tool_call_id: "t1".into(),
                title: Some("Renamed Tool".into()),
                ..Default::default()
            },
        );
        assert!(!agent.tool_results.get("t1").unwrap().is_error);
    }
}
