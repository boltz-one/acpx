//! Ports `others/acpx/src/runtime/engine/reuse-policy.ts`: whether an
//! `ensure_session` call may keep using a persisted record as-is, versus
//! needing a fresh session (which the reconnect state machine or the caller
//! then decides how to handle per [`crate::types::SessionResumePolicy`]).

use crate::session::persistence::absolute_path;
use crate::session::record::SessionRecord;
use std::path::Path;

/// What `ensure_session` asked for, compared against a candidate persisted
/// record to decide reuse eligibility.
pub struct ReuseCandidate<'a> {
    pub cwd: &'a Path,
    pub agent_command: &'a str,
    pub resume_session_id: Option<&'a str>,
}

/// Ports `shouldReuseExistingRecord`: the record's `cwd`/`agentCommand` must
/// match the request, an explicit `resumeSessionId` (if given) must match
/// the record's current backend session id, and the record must not be
/// flagged `reset_on_next_ensure` (set when a prior turn decided the next
/// `ensure_session` must start fresh, e.g. after a fatal replay failure).
pub fn should_reuse_existing_record(
    record: &SessionRecord,
    candidate: &ReuseCandidate<'_>,
) -> bool {
    if record
        .acpx
        .as_ref()
        .and_then(|acpx| acpx.reset_on_next_ensure)
        == Some(true)
    {
        return false;
    }
    if absolute_path(Path::new(&record.cwd)) != absolute_path(candidate.cwd) {
        return false;
    }
    if record.agent_command != candidate.agent_command {
        return false;
    }
    if let Some(resume_session_id) = candidate.resume_session_id
        && record.acp_session_id != resume_session_id
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn matching_cwd_and_agent_reuses_record() {
        let mut record = sample_session_record();
        record.cwd = "/tmp/project".to_string();
        record.agent_command = "claude".to_string();
        let candidate = ReuseCandidate {
            cwd: Path::new("/tmp/project"),
            agent_command: "claude",
            resume_session_id: None,
        };
        assert!(should_reuse_existing_record(&record, &candidate));
    }

    #[test]
    fn mismatched_agent_command_rejects_reuse() {
        let mut record = sample_session_record();
        record.cwd = "/tmp/project".to_string();
        record.agent_command = "claude".to_string();
        let candidate = ReuseCandidate {
            cwd: Path::new("/tmp/project"),
            agent_command: "codex",
            resume_session_id: None,
        };
        assert!(!should_reuse_existing_record(&record, &candidate));
    }

    #[test]
    fn reset_on_next_ensure_forces_fresh_session() {
        let mut record = sample_session_record();
        record.cwd = "/tmp/project".to_string();
        let mut acpx = record.acpx.take().unwrap_or_default();
        acpx.reset_on_next_ensure = Some(true);
        record.acpx = Some(acpx);
        let candidate = ReuseCandidate {
            cwd: Path::new("/tmp/project"),
            agent_command: &record.agent_command.clone(),
            resume_session_id: None,
        };
        assert!(!should_reuse_existing_record(&record, &candidate));
    }
}
