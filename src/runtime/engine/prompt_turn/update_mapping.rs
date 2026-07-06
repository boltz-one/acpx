//! Maps a live typed `SessionUpdate` onto Phase 5's protocol-agnostic
//! [`SessionUpdateInput`] so [`record_session_update`] can fold it into the
//! persisted conversation model. Companion to
//! `runtime::public::events::parse_session_update` (same source, different
//! destination — one produces the live UI event, this produces the
//! persisted-history delta).

use agent_client_protocol::schema::v1::{
    AvailableCommandsUpdate, ContentBlock, CurrentModeUpdate, SessionUpdate, ToolCall,
    ToolCallStatus, ToolCallUpdate, ToolKind, UsageUpdate,
};

use crate::session::acpx_state::SessionAvailableCommand;
use crate::session::conversation_model::agent_content::InboundContent;
use crate::session::conversation_model::conversation::SessionUsageCost;
use crate::session::conversation_model::session_update::SessionUpdateInput;
use crate::session::conversation_model::tool_call::ToolCallUpdateInput;

fn text_from_content_block(block: &ContentBlock) -> Option<&str> {
    match block {
        ContentBlock::Text(text) => Some(text.text.as_str()),
        _ => None,
    }
}

fn kind_str(kind: ToolKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "other".to_string())
}

fn status_str(status: ToolCallStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "pending".to_string())
}

fn tool_call_update_input_from_call(call: &ToolCall) -> ToolCallUpdateInput {
    ToolCallUpdateInput {
        tool_call_id: call.tool_call_id.0.to_string(),
        title: Some(call.title.clone()),
        kind: Some(kind_str(call.kind)),
        status: Some(status_str(call.status)),
        raw_input: call.raw_input.clone(),
        raw_output_present: call.raw_output.is_some(),
        raw_output: call.raw_output.clone(),
    }
}

fn tool_call_update_input_from_update(update: &ToolCallUpdate) -> ToolCallUpdateInput {
    ToolCallUpdateInput {
        tool_call_id: update.tool_call_id.0.to_string(),
        title: update.fields.title.clone(),
        kind: update.fields.kind.map(kind_str),
        status: update.fields.status.map(status_str),
        raw_input: update.fields.raw_input.clone(),
        raw_output_present: update.fields.raw_output.is_some(),
        raw_output: update.fields.raw_output.clone(),
    }
}

fn available_command_from_update(update: &AvailableCommandsUpdate) -> Vec<SessionAvailableCommand> {
    update
        .available_commands
        .iter()
        .map(|command| SessionAvailableCommand {
            name: command.name.clone(),
            description: (!command.description.trim().is_empty())
                .then(|| command.description.clone()),
            has_input: Some(command.input.is_some()),
        })
        .collect()
}

fn current_mode_update_input(update: &CurrentModeUpdate) -> SessionUpdateInput {
    SessionUpdateInput::CurrentModeUpdate(update.current_mode_id.0.to_string())
}

fn usage_update_input(update: &UsageUpdate) -> SessionUpdateInput {
    SessionUpdateInput::UsageUpdate {
        usage: None,
        cost: update.cost.as_ref().map(|cost| SessionUsageCost {
            amount: Some(cost.amount),
            currency: Some(cost.currency.clone()),
        }),
    }
}

/// See module docs.
pub(super) fn session_update_input(update: &SessionUpdate) -> Option<SessionUpdateInput> {
    match update {
        SessionUpdate::UserMessageChunk(chunk) => {
            text_from_content_block(&chunk.content).map(|text| {
                SessionUpdateInput::UserMessageChunk(InboundContent::Text(text.to_string()))
            })
        }
        SessionUpdate::AgentMessageChunk(chunk) => text_from_content_block(&chunk.content)
            .map(|text| SessionUpdateInput::AgentMessageChunk(text.to_string())),
        SessionUpdate::AgentThoughtChunk(chunk) => text_from_content_block(&chunk.content)
            .map(|text| SessionUpdateInput::AgentThoughtChunk(text.to_string())),
        SessionUpdate::ToolCall(call) => Some(SessionUpdateInput::ToolCall(
            tool_call_update_input_from_call(call),
        )),
        SessionUpdate::ToolCallUpdate(update) => Some(SessionUpdateInput::ToolCall(
            tool_call_update_input_from_update(update),
        )),
        SessionUpdate::AvailableCommandsUpdate(update) => Some(
            SessionUpdateInput::AvailableCommandsUpdate(available_command_from_update(update)),
        ),
        SessionUpdate::CurrentModeUpdate(update) => Some(current_mode_update_input(update)),
        SessionUpdate::ConfigOptionUpdate(update) => Some(SessionUpdateInput::ConfigOptionUpdate(
            update.config_options.clone(),
        )),
        SessionUpdate::SessionInfoUpdate(update) => Some(SessionUpdateInput::SessionInfoUpdate {
            title: match &update.title {
                agent_client_protocol::schema::MaybeUndefined::Undefined => None,
                agent_client_protocol::schema::MaybeUndefined::Null => Some(None),
                agent_client_protocol::schema::MaybeUndefined::Value(v) => Some(Some(v.clone())),
            },
            updated_at: update.updated_at.clone().take(),
        }),
        SessionUpdate::UsageUpdate(update) => Some(usage_update_input(update)),
        _ => None,
    }
}
