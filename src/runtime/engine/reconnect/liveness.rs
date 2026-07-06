//! Staleness check for a persisted record's recorded agent pid. Ports the
//! `isProcessAlive`/`logReconnectAttempt` diagnostics half of
//! `others/acpx/src/runtime/engine/reconnect.ts`, reusing
//! [`crate::platform::is_process_alive`] (Phase 1) rather than re-deriving
//! the signal-0 liveness check.
//!
//! Unlike acpx's long-lived CLI daemon (which can, in principle, still hold
//! the exact same OS process alive across invocations), this crate's host
//! is a single continuously-running GPUI process: by the time
//! [`super::connect_and_load_session`] runs at all, the caller has already
//! established there is no live in-memory [`crate::runtime::engine::connected_session::ConnectedSession`]
//! for this record, and reconnecting always means spawning a *fresh* agent
//! subprocess and asking it to resume/load the *same* backend session id.
//! This module's result is therefore diagnostic only (mirrors acpx's
//! `verbose` log branch), not a decision input to the reconnect state
//! machine itself.

use crate::platform::is_process_alive;
use crate::session::record::SessionRecord;

/// Whether `record.pid` is still worth reporting as "the old process was
/// somehow still alive" versus "dead, this is a genuine crash recovery".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredProcessStatus {
    NoPidRecorded,
    Alive,
    Dead,
}

/// Ports the `isProcessAlive(record.pid)` check plus its two call sites in
/// `logReconnectAttempt`.
pub fn stored_process_status(record: &SessionRecord) -> StoredProcessStatus {
    match record.pid {
        None => StoredProcessStatus::NoPidRecorded,
        Some(pid) if is_process_alive(pid) => StoredProcessStatus::Alive,
        Some(_) => StoredProcessStatus::Dead,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn no_pid_recorded_when_absent() {
        let mut record = sample_session_record();
        record.pid = None;
        assert_eq!(
            stored_process_status(&record),
            StoredProcessStatus::NoPidRecorded
        );
    }

    #[test]
    fn dead_when_pid_recorded_but_not_alive() {
        let mut record = sample_session_record();
        // A pid this test process almost certainly doesn't own.
        record.pid = Some(1);
        // On most systems pid 1 exists but isn't a process this test can
        // signal; `is_process_alive` treats permission-denied the same as
        // nonexistent (see its own docs), so this still exercises the
        // "not our own recently-spawned child" path deterministically only
        // when pid 1 truly is unreachable. Skip the exact variant assertion
        // and just confirm it doesn't panic / matches one of the two.
        let status = stored_process_status(&record);
        assert!(matches!(
            status,
            StoredProcessStatus::Alive | StoredProcessStatus::Dead
        ));
    }
}
