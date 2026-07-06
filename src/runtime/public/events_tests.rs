use super::*;
use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandsUpdate, ContentChunk, CurrentModeUpdate, SessionModeId,
    TextContent, ToolCallId, ToolCallUpdateFields, UsageUpdate,
};

#[test]
fn agent_message_chunk_becomes_output_text_delta() {
    let update = SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
        TextContent::new("hello"),
    )));
    let event = parse_session_update(&update).unwrap();
    assert_eq!(
        event,
        AcpRuntimeEvent::TextDelta {
            text: "hello".to_string(),
            stream: AcpRuntimeTextStream::Output,
            tag: Some("agent_message_chunk".to_string()),
        }
    );
}

#[test]
fn agent_thought_chunk_becomes_thought_text_delta() {
    let update = SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(
        TextContent::new("thinking"),
    )));
    let event = parse_session_update(&update).unwrap();
    assert!(matches!(
        event,
        AcpRuntimeEvent::TextDelta {
            stream: AcpRuntimeTextStream::Thought,
            ..
        }
    ));
}

#[test]
fn user_message_chunk_produces_no_event() {
    let update = SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::Text(
        TextContent::new("hi"),
    )));
    assert!(parse_session_update(&update).is_none());
}

#[test]
fn tool_call_carries_id_and_kind() {
    let call = ToolCall::new(ToolCallId::new("call-1"), "Read file.txt").kind(ToolKind::Read);
    let event = parse_session_update(&SessionUpdate::ToolCall(call)).unwrap();
    let AcpRuntimeEvent::ToolCall {
        tool_call_id, kind, ..
    } = event
    else {
        panic!("expected tool_call event");
    };
    assert_eq!(tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(kind, Some(ToolKind::Read));
}

#[test]
fn tool_call_update_defaults_title_when_absent() {
    let update = ToolCallUpdate::new(ToolCallId::new("call-1"), ToolCallUpdateFields::new());
    let event = parse_session_update(&SessionUpdate::ToolCallUpdate(update)).unwrap();
    let AcpRuntimeEvent::ToolCall { text, .. } = event else {
        panic!("expected tool_call event");
    };
    assert!(text.starts_with("tool call"));
}

#[test]
fn current_mode_update_reports_mode_id() {
    let update = CurrentModeUpdate::new(SessionModeId::new("plan"));
    let event = parse_session_update(&SessionUpdate::CurrentModeUpdate(update)).unwrap();
    let AcpRuntimeEvent::Status { text, tag, .. } = event else {
        panic!("expected status event");
    };
    assert_eq!(text, "mode updated: plan");
    assert_eq!(tag.as_deref(), Some("current_mode_update"));
}

#[test]
fn usage_update_carries_used_and_size() {
    let update = UsageUpdate::new(10, 100);
    let event = parse_session_update(&SessionUpdate::UsageUpdate(update)).unwrap();
    let AcpRuntimeEvent::Status { used, size, .. } = event else {
        panic!("expected status event");
    };
    assert_eq!(used, Some(10));
    assert_eq!(size, Some(100));
}

#[test]
fn available_commands_update_lists_commands() {
    let update = AvailableCommandsUpdate::new(vec![AvailableCommand::new(
        "compact",
        "Compact the conversation",
    )]);
    let event = parse_session_update(&SessionUpdate::AvailableCommandsUpdate(update)).unwrap();
    let AcpRuntimeEvent::Status {
        available_commands, ..
    } = event
    else {
        panic!("expected status event");
    };
    let commands = available_commands.unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "compact");
}
