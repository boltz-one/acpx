//! Ports `others/acpx/src/runtime/engine/lifecycle.ts`: folding a live
//! [`AcpClient`]'s process-lifecycle snapshot onto a persisted
//! [`SessionRecord`], plus the agent-session-id reconciliation helper.

use chrono::{DateTime, Utc};

use crate::agent_session_id::normalize_agent_session_id;
use crate::client::state::AgentExitInfo;
use crate::session::conversation_model::SessionConversation;
use crate::session::record::SessionRecord;

/// Ports acpx's `AgentLifecycleSnapshot` (`client.ts`), rebuilt here from
/// this crate's [`crate::client::state::ClientState`] plus a fresh
/// liveness check (acpx's `running` flag is itself derived the same way).
pub struct AgentLifecycleSnapshot {
    pub running: bool,
    pub pid: Option<u32>,
    pub started_at: DateTime<Utc>,
    pub last_exit: Option<AgentExitInfo>,
}

/// Ports `applyLifecycleSnapshotToRecord`.
pub fn apply_lifecycle_snapshot_to_record(
    record: &mut SessionRecord,
    snapshot: Option<&AgentLifecycleSnapshot>,
) {
    let Some(snapshot) = snapshot else {
        return;
    };

    record.pid = snapshot.running.then_some(snapshot.pid).flatten();
    record.agent_started_at = Some(snapshot.started_at.to_rfc3339());

    if let Some(last_exit) = &snapshot.last_exit {
        record.last_agent_exit_code = last_exit.exit_code;
        record.last_agent_exit_signal = last_exit.signal.clone();
        record.last_agent_exit_at = Some(chrono::Utc::now().to_rfc3339());
        record.last_agent_disconnect_reason = Some(last_exit.reason.clone());
        return;
    }

    record.last_agent_exit_code = None;
    record.last_agent_exit_signal = None;
    record.last_agent_exit_at = None;
    record.last_agent_disconnect_reason = None;
}

/// Ports `reconcileAgentSessionId`.
pub fn reconcile_agent_session_id(record: &mut SessionRecord, agent_session_id: Option<&str>) {
    let Some(normalized) = agent_session_id.and_then(|id| normalize_agent_session_id(id)) else {
        return;
    };
    record.agent_session_id = Some(normalized);
}

/// Ports `sessionHasAgentMessages`.
pub fn session_has_agent_messages(conversation: &SessionConversation) -> bool {
    conversation
        .messages
        .iter()
        .any(|message| message.as_agent().is_some())
}

/// Ports `applyConversation`: writes a (possibly mutated) in-memory
/// [`SessionConversation`] back onto the flattened conversation fields
/// [`SessionRecord`] carries directly (this port stores the conversation
/// inline on the record rather than as a nested object, see
/// `session::record`'s module docs).
pub fn apply_conversation(record: &mut SessionRecord, conversation: &SessionConversation) {
    record.title = conversation.title.clone();
    record.updated_at = conversation.updated_at.clone();
    record.messages = conversation.messages.clone();
    record.cumulative_token_usage = conversation.cumulative_token_usage;
    record.cumulative_cost = conversation.cumulative_cost.clone();
    record.request_token_usage = conversation.request_token_usage.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn reconcile_ignores_blank_agent_session_id() {
        let mut record = sample_session_record();
        record.agent_session_id = Some("existing".into());
        reconcile_agent_session_id(&mut record, Some("   "));
        assert_eq!(record.agent_session_id.as_deref(), Some("existing"));
    }

    #[test]
    fn reconcile_sets_normalized_agent_session_id() {
        let mut record = sample_session_record();
        reconcile_agent_session_id(&mut record, Some(" agent-1 "));
        assert_eq!(record.agent_session_id.as_deref(), Some("agent-1"));
    }

    #[test]
    fn apply_snapshot_with_no_exit_clears_prior_exit_fields() {
        let mut record = sample_session_record();
        record.last_agent_exit_code = Some(1);
        let snapshot = AgentLifecycleSnapshot {
            running: true,
            pid: Some(42),
            started_at: Utc::now(),
            last_exit: None,
        };
        apply_lifecycle_snapshot_to_record(&mut record, Some(&snapshot));
        assert_eq!(record.pid, Some(42));
        assert!(record.last_agent_exit_code.is_none());
    }
}
