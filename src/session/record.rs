//! The top-level persisted session record.
//!
//! Ports `SessionRecord` and `SessionImportedFrom` from
//! `others/acpx/src/types.ts`, applying ADR-5's serde strategy (see
//! `plans/20260705-1718-acpx-to-acp-crate-port/phase-05-session-persistence.md`):
//! `#[serde(default)]` on every field acpx treats as optional/defaultable,
//! and a top-level `#[serde(flatten)] extra` catch-all so a future schema
//! addition this struct doesn't know about round-trips unchanged instead of
//! being silently dropped.
//!
//! `agent_capabilities` and `acpx.config_options` use the real
//! `agent_client_protocol` schema types directly, matching acpx's own
//! `types.ts` (which imports `AgentCapabilities`/`SessionConfigOption` from
//! `@agentclientprotocol/sdk` rather than re-deriving their shape).

use agent_client_protocol::schema::v1::AgentCapabilities;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::acpx_state::SessionAcpxState;
use super::conversation_model::{SessionMessage, SessionTokenUsage, SessionUsageCost};
use super::event_log::SessionEventLog;
use super::schema::SessionSchemaVersion;

/// Ports `SessionImportedFrom`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionImportedFrom {
    pub record_id: String,
    pub cwd_original: String,
    pub exported_by: String,
    pub exported_at: String,
}

/// Ports `SessionRecord`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    #[serde(default)]
    pub schema: SessionSchemaVersion,
    pub acpx_record_id: String,
    pub acp_session_id: String,
    #[serde(default)]
    pub agent_session_id: Option<String>,
    pub agent_command: String,
    pub cwd: String,
    #[serde(default)]
    pub name: Option<String>,
    pub created_at: String,
    pub last_used_at: String,
    #[serde(default)]
    pub last_seq: u64,
    #[serde(default)]
    pub last_request_id: Option<String>,
    #[serde(default)]
    pub event_log: SessionEventLog,
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub closed_at: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub agent_started_at: Option<String>,
    #[serde(default)]
    pub last_prompt_at: Option<String>,
    #[serde(default)]
    pub last_agent_exit_code: Option<i32>,
    #[serde(default)]
    pub last_agent_exit_signal: Option<String>,
    #[serde(default)]
    pub last_agent_exit_at: Option<String>,
    #[serde(default)]
    pub last_agent_disconnect_reason: Option<String>,
    #[serde(default)]
    pub protocol_version: Option<i64>,
    #[serde(default)]
    pub agent_capabilities: Option<AgentCapabilities>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub messages: Vec<SessionMessage>,
    pub updated_at: String,
    #[serde(default)]
    pub cumulative_token_usage: SessionTokenUsage,
    #[serde(default)]
    pub cumulative_cost: Option<SessionUsageCost>,
    #[serde(default)]
    pub request_token_usage: IndexMap<String, SessionTokenUsage>,
    #[serde(default)]
    pub acpx: Option<SessionAcpxState>,
    #[serde(default)]
    pub imported_from: Option<SessionImportedFrom>,
    /// Forward-compat catch-all (ADR-5): fields a future version of this
    /// struct doesn't know about yet are preserved here and round-tripped
    /// on next write instead of being dropped.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SessionRecord {
    /// `record.closedAt ?? record.lastUsedAt` — ports `closedAtOrLastUsedAt`
    /// from `session/persistence/repository.ts`, used for prune-cutoff
    /// comparisons.
    pub fn closed_at_or_last_used_at(&self) -> &str {
        self.closed_at.as_deref().unwrap_or(&self.last_used_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_record_json() -> serde_json::Value {
        serde_json::json!({
            "schema": "boltz-acpx.session.v1",
            "acpx_record_id": "r1",
            "acp_session_id": "s1",
            "agent_command": "claude",
            "cwd": "/tmp",
            "created_at": "2026-01-01T00:00:00Z",
            "last_used_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
        })
    }

    #[test]
    fn minimal_record_deserializes_with_defaults() {
        let record: SessionRecord = serde_json::from_value(minimal_record_json()).unwrap();
        assert_eq!(record.last_seq, 0);
        assert!(!record.closed);
        assert!(record.messages.is_empty());
        assert!(record.extra.is_empty());
    }

    #[test]
    fn unknown_top_level_field_is_preserved_via_flatten() {
        let mut json = minimal_record_json();
        json["future_field"] = serde_json::json!("from-a-later-version");
        let record: SessionRecord = serde_json::from_value(json).unwrap();
        assert_eq!(
            record.extra.get("future_field").unwrap(),
            "from-a-later-version"
        );

        let round_tripped = serde_json::to_value(&record).unwrap();
        assert_eq!(round_tripped["future_field"], "from-a-later-version");
    }
}
