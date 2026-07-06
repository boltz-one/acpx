//! Prompt-submission / response-usage recording.
//!
//! Ports the non-live-protocol-typed half of
//! `others/acpx/src/session/conversation-model.ts`: `recordPromptSubmission`,
//! `recordPromptResponseUsage`, `recordClientOperation`,
//! `hasAgentReplyAfterPrompt`. See [`super::agent_content`] for the
//! `appendAgentText`/`appendAgentThinking`/`ensureAgentMessage` helpers and
//! [`super::session_update`] for the tool-call/mode/usage-update analog of
//! `recordSessionUpdate` — split out to stay under this crate's per-file
//! line convention.
//!
//! acpx's `recordPromptSubmission` takes a live `PromptInput` (a
//! `ContentBlock[]` from `@agentclientprotocol/sdk`). This crate's Phase 5
//! depends only on Phase 1 (see plan.md's phase table), so that live-protocol
//! conversion is deliberately not wired here: [`super::agent_content::InboundContent`]
//! is a small, protocol-crate-agnostic stand-in that the runtime engine
//! (Phase 4, which does own the live `agent-client-protocol` connection) is
//! expected to populate before calling these functions.

use super::agent_content::{InboundContent, inbound_to_user_content};
use super::conversation::{SessionConversation, SessionTokenUsage, iso_now};
use super::limits::MAX_RUNTIME_AGENT_TEXT_CHARS;
use super::message::{SessionMessage, SessionUserContent, SessionUserMessage};
use super::trim::{trim_conversation_for_runtime, trim_runtime_text};

fn next_user_message_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn is_user_message(message: &SessionMessage) -> bool {
    matches!(message, SessionMessage::User(_))
}

/// Ports `recordPromptSubmission`. Returns the new user message's id, or
/// `None` if `contents` produced no persistable content (mirrors acpx
/// returning `undefined` when `userContent.length === 0`).
pub fn record_prompt_submission(
    conversation: &mut SessionConversation,
    contents: &[InboundContent],
    timestamp: Option<String>,
) -> Option<String> {
    let user_content: Vec<SessionUserContent> =
        contents.iter().map(inbound_to_user_content).collect();
    if user_content.is_empty() {
        return None;
    }

    let trimmed = user_content
        .into_iter()
        .map(|content| match content {
            SessionUserContent::Text(text) => {
                SessionUserContent::Text(trim_runtime_text(&text, MAX_RUNTIME_AGENT_TEXT_CHARS))
            }
            other => other,
        })
        .collect();

    let id = next_user_message_id();
    conversation
        .messages
        .push(SessionMessage::User(SessionUserMessage {
            id: id.clone(),
            content: trimmed,
        }));
    conversation.updated_at = timestamp.unwrap_or_else(iso_now);
    trim_conversation_for_runtime(conversation);
    Some(id)
}

/// Ports `hasAgentReplyAfterPrompt`.
pub fn has_agent_reply_after_prompt(
    conversation: &SessionConversation,
    prompt_message_id: &str,
) -> bool {
    let mut saw_prompt = false;
    for message in &conversation.messages {
        if !saw_prompt {
            if let SessionMessage::User(user) = message {
                if user.id == prompt_message_id {
                    saw_prompt = true;
                }
            }
            continue;
        }
        if let SessionMessage::Agent(agent) = message {
            if !agent.content.is_empty() || !agent.tool_results.is_empty() {
                return true;
            }
        }
    }
    false
}

/// Ports `recordPromptResponseUsage`. Returns `false` when `usage` carried
/// no recognizable token-usage fields (mirroring acpx's early return).
pub fn record_prompt_response_usage(
    conversation: &mut SessionConversation,
    usage: SessionTokenUsage,
    prompt_message_id: Option<&str>,
    timestamp: Option<String>,
) -> bool {
    if !usage.has_value() {
        return false;
    }
    apply_token_usage(conversation, usage, prompt_message_id);
    conversation.updated_at = timestamp.unwrap_or_else(iso_now);
    trim_conversation_for_runtime(conversation);
    true
}

pub(super) fn apply_token_usage(
    conversation: &mut SessionConversation,
    usage: SessionTokenUsage,
    prompt_message_id: Option<&str>,
) {
    conversation.cumulative_token_usage = usage;
    let user_id = prompt_message_id
        .map(str::to_string)
        .or_else(|| last_user_message_id(conversation));
    if let Some(user_id) = user_id {
        conversation.request_token_usage.insert(user_id, usage);
    }
}

fn last_user_message_id(conversation: &SessionConversation) -> Option<String> {
    conversation
        .messages
        .iter()
        .rev()
        .find(|message| is_user_message(message))
        .and_then(SessionMessage::as_user)
        .map(|user| user.id.clone())
}

/// Ports `recordClientOperation`. Matches acpx exactly: only the timestamp
/// and trim are applied here (the operation itself isn't folded into
/// conversation state — acpx logs it via a separate `onClientOperation`
/// callback outside the persisted record).
pub fn record_client_operation(conversation: &mut SessionConversation, timestamp: Option<String>) {
    conversation.updated_at = timestamp.unwrap_or_else(iso_now);
    trim_conversation_for_runtime(conversation);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::conversation_model::agent_content::{
        append_agent_text, ensure_agent_message,
    };
    use crate::session::conversation_model::conversation::create_session_conversation;

    #[test]
    fn record_prompt_submission_appends_user_message() {
        let mut conversation = create_session_conversation(None);
        let id = record_prompt_submission(
            &mut conversation,
            &[InboundContent::Text("hello".into())],
            Some("2026-01-01T00:00:00Z".into()),
        )
        .unwrap();

        assert_eq!(conversation.messages.len(), 1);
        let user = conversation.messages[0].as_user().unwrap();
        assert_eq!(user.id, id);
        assert_eq!(conversation.updated_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn record_prompt_submission_with_no_content_returns_none() {
        let mut conversation = create_session_conversation(None);
        assert!(record_prompt_submission(&mut conversation, &[], None).is_none());
        assert!(conversation.messages.is_empty());
    }

    #[test]
    fn has_agent_reply_after_prompt_detects_trailing_agent_content() {
        let mut conversation = create_session_conversation(None);
        let id = record_prompt_submission(
            &mut conversation,
            &[InboundContent::Text("hi".into())],
            None,
        )
        .unwrap();
        assert!(!has_agent_reply_after_prompt(&conversation, &id));

        append_agent_text(ensure_agent_message(&mut conversation), "reply");
        assert!(has_agent_reply_after_prompt(&conversation, &id));
    }
}
