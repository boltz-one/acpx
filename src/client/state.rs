//! Non-prompt-queue client lifecycle state. Mirrors the subset of acpx's
//! `client.ts` private fields that describe the agent process's lifecycle
//! (`agentStartedAt`, `lastKnownPid`, `lastAgentExit`) rather than
//! prompt/session bookkeeping (`activePrompt`, `cancellingSessionIds`, etc.
//! — those move to Phase 6's queue per ADR-4, not ported here).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use crate::permissions::PermissionDecisionClass;

/// Why the agent process exited, and how. Ports the fields acpx attaches to
/// `AgentDisconnectedError`/`getAgentLifecycleSnapshot().lastExit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentExitInfo {
    pub reason: String,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
}

/// Aggregate permission-request counters. Ports acpx's `PermissionStats`
/// (`types.ts`), incremented from the `session/request_permission` RPC
/// handler (`recordPermissionDecision`/`recordPermissionError`). Aggregate
/// counts only — no tool names/content — so exposing it carries no
/// sensitive-data surface.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionStats {
    pub requested: u64,
    pub approved: u64,
    pub denied: u64,
    pub cancelled: u64,
}

impl PermissionStats {
    /// Records one resolved permission request: always bumps `requested`,
    /// plus the counter matching the resolved decision's class. Ports
    /// `recordPermissionDecision`.
    pub fn record(&mut self, class: PermissionDecisionClass) {
        self.requested += 1;
        match class {
            PermissionDecisionClass::Approved => self.approved += 1,
            PermissionDecisionClass::Denied => self.denied += 1,
            PermissionDecisionClass::Cancelled => self.cancelled += 1,
        }
    }

    /// Records a request that failed before producing a decision (e.g. the
    /// non-interactive `fail` policy). Only `requested` is bumped. Ports
    /// `recordPermissionError`.
    pub fn record_error(&mut self) {
        self.requested += 1;
    }
}

/// Shared handle to a connection's [`PermissionStats`]: the RPC handler
/// (running on the connection's background task) increments it, and
/// [`super::AcpClient::permission_stats`] reads a snapshot.
pub type PermissionStatsHandle = Arc<Mutex<PermissionStats>>;

/// Lifecycle bookkeeping for one spawned agent process. Deliberately holds
/// no prompt-queue state (see module docs).
#[derive(Debug, Clone)]
pub struct ClientState {
    pub agent_started_at: DateTime<Utc>,
    pub last_known_pid: Option<u32>,
    pub last_agent_exit: Option<AgentExitInfo>,
}

impl ClientState {
    pub fn new(pid: u32) -> Self {
        Self {
            agent_started_at: Utc::now(),
            last_known_pid: Some(pid),
            last_agent_exit: None,
        }
    }

    pub fn record_exit(&mut self, exit: AgentExitInfo) {
        self.last_agent_exit = Some(exit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_has_no_exit_recorded() {
        let state = ClientState::new(1234);
        assert_eq!(state.last_known_pid, Some(1234));
        assert!(state.last_agent_exit.is_none());
    }

    #[test]
    fn record_exit_stores_reason() {
        let mut state = ClientState::new(1);
        state.record_exit(AgentExitInfo {
            reason: "sigterm".to_string(),
            exit_code: None,
            signal: Some("SIGTERM".to_string()),
        });
        assert_eq!(state.last_agent_exit.unwrap().reason, "sigterm");
    }

    #[test]
    fn permission_stats_record_counts_by_class() {
        let mut stats = PermissionStats::default();
        stats.record(PermissionDecisionClass::Approved);
        stats.record(PermissionDecisionClass::Approved);
        stats.record(PermissionDecisionClass::Denied);
        stats.record(PermissionDecisionClass::Cancelled);
        assert_eq!(stats.requested, 4);
        assert_eq!(stats.approved, 2);
        assert_eq!(stats.denied, 1);
        assert_eq!(stats.cancelled, 1);
    }

    #[test]
    fn permission_stats_record_error_only_bumps_requested() {
        let mut stats = PermissionStats::default();
        stats.record_error();
        stats.record(PermissionDecisionClass::Approved);
        assert_eq!(stats.requested, 2);
        assert_eq!(stats.approved, 1);
        assert_eq!(stats.denied, 0);
        assert_eq!(stats.cancelled, 0);
    }
}
