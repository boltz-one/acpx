//! Runtime truncation of a [`SessionConversation`], applied after every
//! mutation so persisted files don't grow unboundedly.
//!
//! Ports `trimConversationForRuntime` and its helpers from
//! `others/acpx/src/session/conversation-model.ts`. Truncation order
//! matches acpx exactly (phase-05 Risk Assessment calls this out
//! explicitly): message-count trim first (drop oldest messages), *then*
//! per-field char-limit trim applied to whatever messages remain, then the
//! `request_token_usage` map is capped last.

use serde_json::Value;

use super::conversation::SessionConversation;
use super::limits::{
    MAX_RUNTIME_AGENT_TEXT_CHARS, MAX_RUNTIME_MESSAGES, MAX_RUNTIME_REQUEST_TOKEN_USAGE,
    MAX_RUNTIME_THINKING_CHARS, MAX_RUNTIME_TOOL_IO_CHARS,
};
use super::message::{
    SessionAgentContent, SessionMessage, SessionToolResultContent, SessionUserContent,
};

/// Ports `trimRuntimeText`: truncates to `max_chars`, appending `...` when
/// truncation actually happened (matching acpx's `slice(0, max - 3) + "..."`
/// including its behavior for `max_chars < 3`).
///
/// `max_chars` counts Unicode scalar values (`.chars()`), *not* UTF-16 code
/// units the way acpx's `string.length`/`slice` does — a divergence for
/// text containing astral-plane characters (e.g. some emoji), which JS
/// counts as 2 UTF-16 units but this counts as 1 scalar value. Exact parity
/// with acpx's truncation point only holds for BMP-only text; for text with
/// astral-plane characters near the truncation boundary this crate will
/// keep slightly more characters (by scalar-value count) than acpx would
/// (by UTF-16-unit count).
pub fn trim_runtime_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let truncated: String = value.chars().take(keep).collect();
    format!("{truncated}...")
}

/// Ports `trimConversationForRuntime`.
pub fn trim_conversation_for_runtime(conversation: &mut SessionConversation) {
    if conversation.messages.len() > MAX_RUNTIME_MESSAGES {
        let drop = conversation.messages.len() - MAX_RUNTIME_MESSAGES;
        conversation.messages.drain(0..drop);
    }

    for message in &mut conversation.messages {
        trim_runtime_message(message);
    }

    if conversation.request_token_usage.len() > MAX_RUNTIME_REQUEST_TOKEN_USAGE {
        // acpx keeps the *last* N entries in insertion order
        // (`Object.entries(...).slice(-N)`). `IndexMap` preserves insertion
        // order, so dropping from the front until only `N` entries remain
        // matches acpx's `slice(-N)` exactly.
        let drop = conversation.request_token_usage.len() - MAX_RUNTIME_REQUEST_TOKEN_USAGE;
        for _ in 0..drop {
            conversation.request_token_usage.shift_remove_index(0);
        }
    }
}

fn trim_runtime_message(message: &mut SessionMessage) {
    match message {
        SessionMessage::User(user) => {
            for content in &mut user.content {
                if let SessionUserContent::Text(text) = content {
                    *text = trim_runtime_text(text, MAX_RUNTIME_AGENT_TEXT_CHARS);
                }
            }
        }
        SessionMessage::Agent(agent) => {
            for content in &mut agent.content {
                trim_runtime_agent_content(content);
            }
            for result in agent.tool_results.values_mut() {
                if let SessionToolResultContent::Text(text) = &mut result.content {
                    *text = trim_runtime_text(text, MAX_RUNTIME_TOOL_IO_CHARS);
                }
                // Ports acpx's `typeof result.output === "string"` guard in
                // `trimRuntimeToolResult`: only a raw string `output` is
                // trimmed; object/array outputs are left byte-for-byte
                // unchanged (gap 17).
                if let Some(Value::String(text)) = &result.output {
                    result.output = Some(Value::String(trim_runtime_text(
                        text,
                        MAX_RUNTIME_TOOL_IO_CHARS,
                    )));
                }
            }
        }
        SessionMessage::Resume => {}
    }
}

fn trim_runtime_agent_content(content: &mut SessionAgentContent) {
    match content {
        SessionAgentContent::Text(text) => {
            *text = trim_runtime_text(text, MAX_RUNTIME_AGENT_TEXT_CHARS);
        }
        SessionAgentContent::Thinking { text, .. } => {
            *text = trim_runtime_text(text, MAX_RUNTIME_THINKING_CHARS);
        }
        SessionAgentContent::ToolUse(tool_use) => {
            tool_use.raw_input = trim_runtime_text(&tool_use.raw_input, MAX_RUNTIME_TOOL_IO_CHARS);
        }
        SessionAgentContent::RedactedThinking(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::conversation_model::conversation::create_session_conversation;
    use crate::session::conversation_model::message::{
        SessionAgentMessage, SessionToolResult, SessionUserMessage,
    };

    #[test]
    fn trim_runtime_text_truncates_and_appends_ellipsis() {
        let long = "a".repeat(10);
        assert_eq!(trim_runtime_text(&long, 5), "aa...");
        assert_eq!(trim_runtime_text("short", 10), "short");
    }

    #[test]
    fn caps_message_count_at_200_dropping_oldest() {
        let mut conversation = create_session_conversation(None);
        for i in 0..250 {
            conversation
                .messages
                .push(SessionMessage::User(SessionUserMessage {
                    id: format!("m{i}"),
                    content: vec![SessionUserContent::Text(format!("msg-{i}"))],
                }));
        }

        trim_conversation_for_runtime(&mut conversation);

        assert_eq!(conversation.messages.len(), MAX_RUNTIME_MESSAGES);
        // Oldest 50 messages (m0..m49) were dropped; m50 now leads.
        let first = conversation.messages.first().unwrap().as_user().unwrap();
        assert_eq!(first.id, "m50");
        let last = conversation.messages.last().unwrap().as_user().unwrap();
        assert_eq!(last.id, "m249");
    }

    #[test]
    fn trims_oversized_agent_text_message() {
        let mut conversation = create_session_conversation(None);
        let oversized = "x".repeat(MAX_RUNTIME_AGENT_TEXT_CHARS + 500);
        conversation
            .messages
            .push(SessionMessage::Agent(SessionAgentMessage {
                content: vec![SessionAgentContent::Text(oversized)],
                ..Default::default()
            }));

        trim_conversation_for_runtime(&mut conversation);

        let agent = conversation.messages[0].as_agent().unwrap();
        let SessionAgentContent::Text(text) = &agent.content[0] else {
            panic!("expected Text content");
        };
        assert_eq!(text.chars().count(), MAX_RUNTIME_AGENT_TEXT_CHARS);
        assert!(text.ends_with("..."));
    }

    #[test]
    fn caps_request_token_usage_keeping_last_n_by_insertion_order() {
        use crate::session::conversation_model::conversation::SessionTokenUsage;

        let mut conversation = create_session_conversation(None);
        for i in 0..150 {
            conversation.request_token_usage.insert(
                format!("req-{i}"),
                SessionTokenUsage {
                    total_tokens: Some(i as u64),
                    ..Default::default()
                },
            );
        }

        trim_conversation_for_runtime(&mut conversation);

        assert_eq!(
            conversation.request_token_usage.len(),
            MAX_RUNTIME_REQUEST_TOKEN_USAGE
        );
        // Exactly entries 50..150 (the last 100 inserted), in original
        // insertion order — matches acpx's `Object.entries(...).slice(-100)`.
        let expected_keys: Vec<String> = (50..150).map(|i| format!("req-{i}")).collect();
        let actual_keys: Vec<String> = conversation.request_token_usage.keys().cloned().collect();
        assert_eq!(actual_keys, expected_keys);
    }

    #[test]
    fn trims_oversized_string_tool_result_output_leaves_object_output_untouched() {
        let mut conversation = create_session_conversation(None);
        let oversized = "y".repeat(MAX_RUNTIME_TOOL_IO_CHARS + 100);
        let object_output = serde_json::json!({"path": "a.txt", "unchanged": true});

        let mut agent = SessionAgentMessage::default();
        agent.tool_results.insert(
            "string-output".to_string(),
            SessionToolResult {
                tool_use_id: "string-output".to_string(),
                tool_name: "tool_call".to_string(),
                is_error: false,
                content: SessionToolResultContent::Text(String::new()),
                output: Some(Value::String(oversized.clone())),
            },
        );
        agent.tool_results.insert(
            "object-output".to_string(),
            SessionToolResult {
                tool_use_id: "object-output".to_string(),
                tool_name: "tool_call".to_string(),
                is_error: false,
                content: SessionToolResultContent::Text(String::new()),
                output: Some(object_output.clone()),
            },
        );
        conversation.messages.push(SessionMessage::Agent(agent));

        trim_conversation_for_runtime(&mut conversation);

        let agent = conversation.messages[0].as_agent().unwrap();
        let string_output = agent.tool_results.get("string-output").unwrap();
        let Some(Value::String(trimmed)) = &string_output.output else {
            panic!("expected string output");
        };
        assert_eq!(trimmed.chars().count(), MAX_RUNTIME_TOOL_IO_CHARS);
        assert!(trimmed.ends_with("..."));

        let object_result = agent.tool_results.get("object-output").unwrap();
        assert_eq!(object_result.output, Some(object_output));
    }
}
