//! The `SessionUpdate` dispatch table.
//!
//! Ports `applySessionUpdate`/`SESSION_UPDATE_HANDLERS`/`recordSessionUpdate`
//! from `others/acpx/src/session/conversation-model.ts`, restricted to
//! updates this crate can express without a live protocol dependency (see
//! [`super::record`]'s module docs — Phase 5 depends only on Phase 1).
//! [`SessionUpdateInput`] is a protocol-crate-agnostic stand-in the runtime
//! engine (Phase 4) is expected to populate from the real
//! `agent_client_protocol` notification types. Tool-call application itself
//! lives in [`super::tool_call`] (split out to stay under this crate's
//! per-file line convention).

use super::agent_content::{
    InboundContent, append_agent_text, append_agent_thinking, ensure_agent_message,
    inbound_to_user_content,
};
use super::conversation::{SessionConversation, SessionTokenUsage, SessionUsageCost, iso_now};
use super::message::{SessionMessage, SessionUserMessage};
use super::record::apply_token_usage;
use super::tool_call::{ToolCallUpdateInput, apply_tool_call_update};
use super::trim::trim_conversation_for_runtime;
use crate::session::acpx_state::{SessionAcpxState, SessionAvailableCommand};

pub enum SessionUpdateInput {
    UserMessageChunk(InboundContent),
    AgentMessageChunk(String),
    AgentThoughtChunk(String),
    ToolCall(ToolCallUpdateInput),
    AvailableCommandsUpdate(Vec<SessionAvailableCommand>),
    CurrentModeUpdate(String),
    ConfigOptionUpdate(Vec<agent_client_protocol::schema::v1::SessionConfigOption>),
    SessionInfoUpdate {
        title: Option<Option<String>>,
        updated_at: Option<String>,
    },
    UsageUpdate {
        usage: Option<SessionTokenUsage>,
        cost: Option<SessionUsageCost>,
    },
}

/// Ports `recordSessionUpdate`.
pub fn record_session_update(
    conversation: &mut SessionConversation,
    acpx: &mut SessionAcpxState,
    update: SessionUpdateInput,
    timestamp: Option<String>,
) {
    match update {
        SessionUpdateInput::UserMessageChunk(content) => {
            let user_content = inbound_to_user_content(&content);
            conversation
                .messages
                .push(SessionMessage::User(SessionUserMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    content: vec![user_content],
                }));
        }
        SessionUpdateInput::AgentMessageChunk(text) => {
            append_agent_text(ensure_agent_message(conversation), &text);
        }
        SessionUpdateInput::AgentThoughtChunk(text) => {
            append_agent_thinking(ensure_agent_message(conversation), &text);
        }
        SessionUpdateInput::ToolCall(tool_update) => {
            apply_tool_call_update(ensure_agent_message(conversation), &tool_update);
        }
        SessionUpdateInput::AvailableCommandsUpdate(commands) => {
            acpx.available_commands = Some(commands);
        }
        SessionUpdateInput::CurrentModeUpdate(mode_id) => {
            acpx.current_mode_id = Some(mode_id);
        }
        SessionUpdateInput::ConfigOptionUpdate(config_options) => {
            crate::session::model_state::apply_config_options_model_state(acpx, config_options);
        }
        SessionUpdateInput::SessionInfoUpdate { title, updated_at } => {
            if let Some(title) = title {
                conversation.title = title;
            }
            if let Some(updated_at) = updated_at {
                conversation.updated_at = updated_at;
            }
        }
        SessionUpdateInput::UsageUpdate { usage, cost } => {
            if let Some(usage) = usage {
                apply_token_usage(conversation, usage, None);
            }
            if let Some(cost) = cost {
                conversation.cumulative_cost = Some(cost);
            }
        }
    }

    conversation.updated_at = timestamp.unwrap_or_else(iso_now);
    trim_conversation_for_runtime(conversation);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::conversation_model::conversation::create_session_conversation;

    #[test]
    fn user_message_chunk_appends_a_user_message() {
        let mut conversation = create_session_conversation(None);
        let mut acpx = SessionAcpxState::default();
        record_session_update(
            &mut conversation,
            &mut acpx,
            SessionUpdateInput::UserMessageChunk(InboundContent::Text("hi".into())),
            None,
        );
        assert_eq!(conversation.messages.len(), 1);
        assert!(conversation.messages[0].as_user().is_some());
    }

    #[test]
    fn agent_message_chunk_appends_to_trailing_agent_message() {
        let mut conversation = create_session_conversation(None);
        let mut acpx = SessionAcpxState::default();
        record_session_update(
            &mut conversation,
            &mut acpx,
            SessionUpdateInput::AgentMessageChunk("hello".into()),
            None,
        );
        record_session_update(
            &mut conversation,
            &mut acpx,
            SessionUpdateInput::AgentMessageChunk(" world".into()),
            None,
        );
        assert_eq!(conversation.messages.len(), 1);
    }

    #[test]
    fn current_mode_update_sets_acpx_state() {
        let mut conversation = create_session_conversation(None);
        let mut acpx = SessionAcpxState::default();
        record_session_update(
            &mut conversation,
            &mut acpx,
            SessionUpdateInput::CurrentModeUpdate("plan".into()),
            None,
        );
        assert_eq!(acpx.current_mode_id.as_deref(), Some("plan"));
    }
}
