//! SIGTERM(grace)->SIGKILL(grace) escalation over a terminal's whole process
//! group. Mirrors Phase 2's `client::shutdown` sequence
//! (`control::with_timeout` + group signalling instead of a single pid).

use std::time::Duration;

use super::ManagedTerminal;
use super::tracking;
use crate::control::with_timeout;

/// Matches acpx's `DEFAULT_KILL_GRACE_MS` (Phase 2's client shutdown uses
/// the same 1.5s SIGTERM grace).
pub const DEFAULT_KILL_GRACE: Duration = Duration::from_millis(1_500);

pub async fn kill_process(terminal: &ManagedTerminal, grace: Duration) {
    if tracking::current_exit_status(&terminal.child)
        .await
        .is_some()
    {
        return;
    }

    tracking::send_group_signal(terminal.pid, tracking::SIGTERM);
    if with_timeout(wait_for_group_exit(terminal), Some(grace))
        .await
        .is_ok()
    {
        return;
    }

    tracking::send_group_signal(terminal.pid, tracking::SIGKILL);
    #[cfg(windows)]
    {
        // `killpg` is a no-op on Windows (see `tracking` module docs);
        // fall back to `Child::kill()`'s single-process TerminateProcess.
        let mut child = terminal.child.lock().await;
        let _ = child.kill();
    }
    let _ = with_timeout(wait_for_group_exit(terminal), Some(grace)).await;
}

async fn wait_for_group_exit(terminal: &ManagedTerminal) {
    tracking::poll_exit_status(&terminal.child).await;
    while tracking::has_live_process_group(terminal.pid) {
        smol::Timer::after(Duration::from_millis(25)).await;
    }
}
