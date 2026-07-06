//! Trailing-`Agent`-message append helpers and the protocol-crate-agnostic
//! inbound-content stand-in.
//!
//! Ports `ensureAgentMessage`, `appendAgentText`, `appendAgentThinking`, and
//! `contentToUserContent` from
//! `others/acpx/src/session/conversation-model.ts`. See [`super::record`]'s
//! module docs for why [`InboundContent`] isn't the real ACP `ContentBlock`
//! type.

use super::limits::{MAX_RUNTIME_AGENT_TEXT_CHARS, MAX_RUNTIME_THINKING_CHARS};
use super::message::{
    SessionAgentContent, SessionAgentMessage, SessionMessage, SessionMessageAudio,
    SessionMessageImage, SessionUserContent,
};
use super::trim::trim_runtime_text;
use crate::session::conversation_model::conversation::SessionConversation;

/// Protocol-crate-agnostic stand-in for an ACP `ContentBlock`, covering the
/// variants acpx's `contentToUserContent` maps to a [`SessionUserContent`].
#[derive(Debug, Clone, PartialEq)]
pub enum InboundContent {
    Text(String),
    Mention { uri: String, content: String },
    Image { source: String },
    Audio { source: String, mime_type: String },
}

pub(super) fn inbound_to_user_content(content: &InboundContent) -> SessionUserContent {
    match content {
        InboundContent::Text(text) => SessionUserContent::Text(text.clone()),
        InboundContent::Mention { uri, content } => SessionUserContent::Mention {
            uri: uri.clone(),
            content: content.clone(),
        },
        InboundContent::Image { source } => SessionUserContent::Image(SessionMessageImage {
            source: source.clone(),
            size: None,
        }),
        InboundContent::Audio { source, mime_type } => {
            SessionUserContent::Audio(SessionMessageAudio {
                source: source.clone(),
                mime_type: mime_type.clone(),
            })
        }
    }
}

/// Ports `ensureAgentMessage`: returns the trailing `Agent` message if the
/// conversation already ends with one, otherwise appends a fresh one.
pub fn ensure_agent_message(conversation: &mut SessionConversation) -> &mut SessionAgentMessage {
    let needs_new = !matches!(conversation.messages.last(), Some(SessionMessage::Agent(_)));
    if needs_new {
        conversation
            .messages
            .push(SessionMessage::Agent(SessionAgentMessage::default()));
    }
    conversation
        .messages
        .last_mut()
        .and_then(SessionMessage::as_agent_mut)
        .expect("just ensured a trailing Agent message")
}

/// Ports `appendAgentText`.
pub fn append_agent_text(agent: &mut SessionAgentMessage, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if let Some(SessionAgentContent::Text(existing)) = agent.content.last_mut() {
        *existing = trim_runtime_text(&format!("{existing}{text}"), MAX_RUNTIME_AGENT_TEXT_CHARS);
        return;
    }
    agent
        .content
        .push(SessionAgentContent::Text(text.to_string()));
}

/// Ports `appendAgentThinking`.
pub fn append_agent_thinking(agent: &mut SessionAgentMessage, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if let Some(SessionAgentContent::Thinking { text: existing, .. }) = agent.content.last_mut() {
        *existing = trim_runtime_text(&format!("{existing}{text}"), MAX_RUNTIME_THINKING_CHARS);
        return;
    }
    agent.content.push(SessionAgentContent::Thinking {
        text: text.to_string(),
        signature: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_agent_text_merges_with_trailing_text_content() {
        let mut agent = SessionAgentMessage::default();
        append_agent_text(&mut agent, "hello ");
        append_agent_text(&mut agent, "world");
        assert_eq!(agent.content.len(), 1);
        let SessionAgentContent::Text(text) = &agent.content[0] else {
            panic!("expected Text content");
        };
        assert_eq!(text, "hello world");
    }

    #[test]
    fn append_agent_thinking_merges_with_trailing_thinking_content() {
        let mut agent = SessionAgentMessage::default();
        append_agent_thinking(&mut agent, "step one. ");
        append_agent_thinking(&mut agent, "step two.");
        assert_eq!(agent.content.len(), 1);
        let SessionAgentContent::Thinking { text, .. } = &agent.content[0] else {
            panic!("expected Thinking content");
        };
        assert_eq!(text, "step one. step two.");
    }

    #[test]
    fn ensure_agent_message_reuses_trailing_agent_message() {
        let mut conversation = super::super::conversation::create_session_conversation(None);
        ensure_agent_message(&mut conversation);
        ensure_agent_message(&mut conversation);
        assert_eq!(conversation.messages.len(), 1);
    }
}
