//! Canonicalizes a [`SessionRecord`] into the exact `serde_json::Value`
//! written to disk.
//!
//! Ports `others/acpx/src/session/persistence/serialize.ts`. Most of that
//! file's job (renaming camelCase TS fields to snake_case JSON keys) is
//! unnecessary here — [`SessionRecord`]'s fields are already snake_case, so
//! a plain `serde_json::to_value` produces the right shape structurally
//! (ADR-5). What's left, and what this module actually does, is the two
//! non-structural fixups acpx's `serializeSessionRecordForDisk` applies:
//! forcing the schema tag to the current version regardless of what's on
//! the in-memory record, and normalizing `agent_session_id`.

use serde_json::Value;

use crate::session::record::SessionRecord;
use crate::session::schema::SessionSchemaVersion;

/// Ports `serializeSessionRecordForDisk`.
pub fn serialize_session_record_for_disk(record: &SessionRecord) -> Value {
    let mut canonical = record.clone();
    canonical.schema = SessionSchemaVersion::V1;
    canonical.agent_session_id = canonical
        .agent_session_id
        .as_deref()
        .and_then(crate::agent_session_id::normalize_agent_session_id);
    serde_json::to_value(&canonical).expect("SessionRecord always serializes to a JSON object")
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::HashMap;

    use indexmap::IndexMap;

    use crate::session::acpx_state::SessionAcpxState;
    use crate::session::conversation_model::{
        SessionAgentContent, SessionAgentMessage, SessionMessage, SessionToolResult,
        SessionToolResultContent, SessionToolUse, SessionUserContent, SessionUserMessage,
    };
    use crate::session::event_log::default_session_event_log;
    use crate::session::record::SessionRecord;
    use crate::session::schema::SessionSchemaVersion;
    use crate::session::store_options::AcpFileSessionStoreOptions;

    /// A [`SessionRecord`] exercising every message/content variant, used by
    /// round-trip and persisted-key-policy regression tests.
    pub(crate) fn sample_session_record() -> SessionRecord {
        let options = AcpFileSessionStoreOptions::new("/tmp/boltz-acpx-test");
        let mut tool_results = HashMap::new();
        tool_results.insert(
            "call-1".to_string(),
            SessionToolResult {
                tool_use_id: "call-1".to_string(),
                tool_name: "read_file".to_string(),
                is_error: false,
                content: SessionToolResultContent::Text("file contents".to_string()),
                output: None,
            },
        );

        SessionRecord {
            schema: SessionSchemaVersion::V1,
            acpx_record_id: "record-1".to_string(),
            acp_session_id: "session-1".to_string(),
            agent_session_id: Some(" agent-session-1 ".to_string()),
            agent_command: "claude".to_string(),
            cwd: "/tmp/project".to_string(),
            name: Some("demo".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_used_at: "2026-01-01T00:00:00Z".to_string(),
            last_seq: 2,
            last_request_id: Some("req-1".to_string()),
            event_log: default_session_event_log(&options, "record-1"),
            closed: false,
            closed_at: None,
            // Deliberately `None`: this fixture is reused by
            // `repository::write_session_record`/`resolve_session_record`
            // tests, and `close_session` sends a real OS kill signal to
            // `record.pid` if set — an arbitrary placeholder pid here could
            // collide with an unrelated live process on the test machine.
            pid: None,
            agent_started_at: Some("2026-01-01T00:00:00Z".to_string()),
            last_prompt_at: Some("2026-01-01T00:00:01Z".to_string()),
            last_agent_exit_code: None,
            last_agent_exit_signal: None,
            last_agent_exit_at: None,
            last_agent_disconnect_reason: None,
            protocol_version: Some(1),
            agent_capabilities: None,
            title: Some("Demo session".to_string()),
            messages: vec![
                SessionMessage::User(SessionUserMessage {
                    id: "u1".to_string(),
                    content: vec![SessionUserContent::Text("hello".to_string())],
                }),
                SessionMessage::Agent(SessionAgentMessage {
                    content: vec![
                        SessionAgentContent::Text("hi there".to_string()),
                        SessionAgentContent::ToolUse(SessionToolUse {
                            id: "call-1".to_string(),
                            name: "read_file".to_string(),
                            raw_input: "{\"path\":\"a.txt\"}".to_string(),
                            input: serde_json::json!({"path": "a.txt"}),
                            is_input_complete: true,
                            thought_signature: None,
                        }),
                    ],
                    tool_results,
                    reasoning_details: None,
                }),
                SessionMessage::Resume,
            ],
            updated_at: "2026-01-01T00:00:02Z".to_string(),
            cumulative_token_usage: Default::default(),
            cumulative_cost: None,
            request_token_usage: IndexMap::new(),
            acpx: Some(SessionAcpxState::default()),
            imported_from: None,
            extra: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forces_canonical_schema_and_trims_agent_session_id() {
        let record = test_support::sample_session_record();
        let value = serialize_session_record_for_disk(&record);
        assert_eq!(value["schema"], "boltz-acpx.session.v1");
        assert_eq!(value["agent_session_id"], "agent-session-1");
    }

    #[test]
    fn every_field_is_present_snake_case() {
        let record = test_support::sample_session_record();
        let value = serialize_session_record_for_disk(&record);
        for key in [
            "acpx_record_id",
            "acp_session_id",
            "agent_command",
            "cwd",
            "created_at",
            "last_used_at",
            "last_seq",
            "event_log",
            "messages",
            "updated_at",
            "cumulative_token_usage",
            "request_token_usage",
        ] {
            assert!(value.get(key).is_some(), "missing key: {key}");
        }
    }
}
