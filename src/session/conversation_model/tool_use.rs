//! `ToolUse` content lookup/creation and tool-result upsert.
//!
//! Ports `ensureToolUseContent`/`upsertToolResult`/`toRawInput`/
//! `toToolResultContent` from
//! `others/acpx/src/session/conversation-model.ts`.

use serde_json::Value;

use super::limits::MAX_RUNTIME_TOOL_IO_CHARS;
use super::message::{
    SessionAgentContent, SessionAgentMessage, SessionToolResult, SessionToolResultContent,
    SessionToolUse,
};
use super::trim::trim_runtime_text;

pub(super) fn to_raw_input(value: Option<&Value>) -> String {
    match value {
        // acpx's `toRawInput` special-cases `typeof value === "string"` to
        // return the raw string (trimmed), never re-encoding it through
        // `JSON.stringify` (gap 18) — `Value::to_string()` would otherwise
        // double-quote it (`"abc"` -> `"\"abc\""`).
        Some(Value::String(text)) => trim_runtime_text(text, MAX_RUNTIME_TOOL_IO_CHARS),
        Some(value) => trim_runtime_text(&value.to_string(), MAX_RUNTIME_TOOL_IO_CHARS),
        None => trim_runtime_text("{}", MAX_RUNTIME_TOOL_IO_CHARS),
    }
}

pub(super) fn to_tool_result_content(value: Option<&Value>) -> SessionToolResultContent {
    match value {
        None => SessionToolResultContent::Text(String::new()),
        Some(Value::String(text)) => {
            SessionToolResultContent::Text(trim_runtime_text(text, MAX_RUNTIME_TOOL_IO_CHARS))
        }
        Some(other) => SessionToolResultContent::Text(trim_runtime_text(
            &other.to_string(),
            MAX_RUNTIME_TOOL_IO_CHARS,
        )),
    }
}

/// Ports `ensureToolUseContent`.
pub(super) fn ensure_tool_use_content<'a>(
    agent: &'a mut SessionAgentMessage,
    tool_call_id: &str,
) -> &'a mut SessionToolUse {
    let index = agent.content.iter().position(
        |content| matches!(content, SessionAgentContent::ToolUse(tool) if tool.id == tool_call_id),
    );
    let index = index.unwrap_or_else(|| {
        agent
            .content
            .push(SessionAgentContent::ToolUse(SessionToolUse {
                id: tool_call_id.to_string(),
                name: "tool_call".to_string(),
                raw_input: "{}".to_string(),
                input: Value::Object(Default::default()),
                is_input_complete: false,
                thought_signature: None,
            }));
        agent.content.len() - 1
    });
    match &mut agent.content[index] {
        SessionAgentContent::ToolUse(tool) => tool,
        _ => unreachable!("index was located via a ToolUse match"),
    }
}

/// Ports `upsertToolResult`. `is_error` is a concrete `bool`, not
/// `Option<bool>` — this crate's only call site (`apply_tool_call_update`)
/// always derives a concrete value from `status_indicates_error` per ADR-9
/// (gap 19), so there's no "preserve prior value" case to model here. If a
/// future caller genuinely needs sticky-preserve semantics, reintroduce
/// `Option<bool>` deliberately with its own ADR rather than silently
/// restoring it.
pub(super) fn upsert_tool_result(
    agent: &mut SessionAgentMessage,
    tool_call_id: &str,
    tool_name: Option<String>,
    is_error: bool,
    content: Option<SessionToolResultContent>,
    output: Option<Value>,
) {
    let fallback = agent
        .tool_results
        .get(tool_call_id)
        .cloned()
        .unwrap_or(SessionToolResult {
            tool_use_id: String::new(),
            tool_name: "tool_call".to_string(),
            is_error: false,
            content: SessionToolResultContent::Text(String::new()),
            output: None,
        });
    agent.tool_results.insert(
        tool_call_id.to_string(),
        SessionToolResult {
            tool_use_id: tool_call_id.to_string(),
            tool_name: tool_name.unwrap_or(fallback.tool_name),
            is_error,
            content: content.unwrap_or(fallback.content),
            output: output.or(fallback.output),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_raw_input_string_value_round_trips_without_extra_quotes() {
        let value = Value::String("abc".to_string());
        assert_eq!(to_raw_input(Some(&value)), "abc");
    }

    #[test]
    fn to_raw_input_object_value_is_json_encoded() {
        let value = serde_json::json!({"path": "a.txt"});
        assert_eq!(to_raw_input(Some(&value)), value.to_string());
    }

    #[test]
    fn to_raw_input_none_defaults_to_empty_object() {
        assert_eq!(to_raw_input(None), "{}");
    }
}
