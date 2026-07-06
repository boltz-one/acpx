//! `session/update` -> [`AcpRuntimeEvent`] parsing.
//!
//! Ports `others/acpx/src/runtime/public/events.ts`'s *event shapes*
//! faithfully (see [`super::types`]), but not its *parsing mechanism*: acpx
//! parses raw NDJSON text lines (`parsePromptEventLine(line: string)`)
//! because its only view of a `session/update` notification is whatever
//! un-typed JSON its hand-rolled transport handed it. This crate receives
//! the real, typed `agent_client_protocol_schema::v1::SessionUpdate` enum
//! from the SDK (per ADR-1 — reuse the SDK for everything it covers), so
//! [`parse_session_update`] pattern-matches that enum directly instead of
//! re-deriving JSON field sniffing.
//!
//! Split (per the workspace's <200-line file guideline) into this
//! dispatcher plus [`tool_call`] (`ToolCall`/`ToolCallUpdate` -> event) and
//! [`status`] (plan/usage/available-commands/generic -> `Status` event).

mod status;
mod tool_call;

use agent_client_protocol::schema::v1::{ContentBlock, SessionUpdate};
// `ToolCall`/`ToolCallUpdate`/`ToolKind` aren't referenced directly in this
// file, but `events_tests.rs` (included below via `super::*`) constructs
// them directly.
#[cfg(test)]
use agent_client_protocol::schema::v1::{ToolCall, ToolCallUpdate, ToolKind};

use super::types::{AcpRuntimeEvent, AcpRuntimeTextStream};
use status::{
    available_commands_event, plan_status_text, status_event, usage_breakdown_from_meta, usage_cost,
};
use tool_call::{tool_call_event, tool_call_update_event};

fn text_from_content_block(block: &ContentBlock) -> Option<&str> {
    match block {
        ContentBlock::Text(text) => Some(text.text.as_str()),
        _ => None,
    }
}

fn text_delta(text: &str, stream: AcpRuntimeTextStream, tag: &str) -> Option<AcpRuntimeEvent> {
    if text.is_empty() {
        return None;
    }
    Some(AcpRuntimeEvent::TextDelta {
        text: text.to_string(),
        stream,
        tag: Some(tag.to_string()),
    })
}

/// Ports `parsePromptEventLine`, adapted to operate on the typed
/// `SessionUpdate` this crate actually receives (see module docs).
/// `UserMessageChunk` deliberately yields `None`: acpx's own dispatch table
/// has no entry for it either (a user-message echo isn't surfaced as a live
/// runtime event).
pub fn parse_session_update(update: &SessionUpdate) -> Option<AcpRuntimeEvent> {
    match update {
        SessionUpdate::UserMessageChunk(_) => None,
        SessionUpdate::AgentMessageChunk(chunk) => {
            let text = text_from_content_block(&chunk.content)?;
            text_delta(text, AcpRuntimeTextStream::Output, "agent_message_chunk")
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            let text = text_from_content_block(&chunk.content)?;
            text_delta(text, AcpRuntimeTextStream::Thought, "agent_thought_chunk")
        }
        SessionUpdate::ToolCall(call) => Some(tool_call_event(call)),
        SessionUpdate::ToolCallUpdate(update) => Some(tool_call_update_event(update)),
        SessionUpdate::Plan(plan) => plan_status_text(plan),
        SessionUpdate::AvailableCommandsUpdate(update) => Some(available_commands_event(update)),
        SessionUpdate::CurrentModeUpdate(update) => Some(status_event(
            "current_mode_update",
            format!("mode updated: {}", update.current_mode_id.0),
        )),
        SessionUpdate::ConfigOptionUpdate(update) => Some(status_event(
            "config_option_update",
            format!("config updated ({})", update.config_options.len()),
        )),
        SessionUpdate::SessionInfoUpdate(update) => {
            let text = update
                .title
                .clone()
                .take()
                .unwrap_or_else(|| "session updated".to_string());
            Some(status_event("session_info_update", text))
        }
        SessionUpdate::UsageUpdate(update) => {
            let text = format!("usage updated: {}/{}", update.used, update.size);
            Some(AcpRuntimeEvent::Status {
                text,
                tag: Some("usage_update".to_string()),
                used: Some(update.used),
                size: Some(update.size),
                cost: usage_cost(update.cost.as_ref()),
                breakdown: usage_breakdown_from_meta(update.meta.as_ref()),
                available_commands: None,
            })
        }
        // `SessionUpdate` is `#[non_exhaustive]`: a future ACP schema
        // revision may add variants (e.g. `PlanUpdate`/`PlanRemoved` behind
        // the `unstable_plan_operations` feature, not enabled here). Treat
        // anything this crate doesn't recognize as "no live event", the
        // same default acpx's dispatch table gives an unmatched tag.
        _ => None,
    }
}

// Split out per the workspace's <200-line file guideline; logically still
// part of this module (`super::*` sees its private items).
#[cfg(test)]
#[path = "../../events_tests.rs"]
mod tests;
