//! Process-group liveness + signalling for the SIGTERM->SIGKILL kill
//! escalation. Ports the relevant slice of `terminal-manager.ts`'s
//! `hasLiveProcessGroup`/`signalPosixProcessGroup`.
//!
//! Full descendant-PID snapshotting (the `ps -eo pid=,ppid=` tree walk in
//! the TS source, ~150 lines) is deliberately **not** ported: this crate's
//! terminal spawn (see `terminal::spawn`) always goes through
//! `crate::util::process::Child::spawn`, which (per ADR-3,
//! `set_pre_exec_to_start_new_session`) already starts every terminal
//! command as a new POSIX session leader. That process's own process group
//! therefore already contains every descendant that hasn't independently
//! called `setpgid` — one `killpg` reaps the whole tree without a separate
//! tracking pass. This resolves the phase's unresolved question #6 (see
//! plan.md) in favor of "no v1 descendant-pid tracking needed" on POSIX.
//!
//! Windows has no equivalent to a POSIX process group; `crate::util::process`
//! already documents that gap (no job-object cleanup — see its own
//! `TODO(windows)`), so a killed terminal's grandchildren may survive on
//! Windows here too, same as Phase 2's ADR-3 Risk Assessment already notes
//! for the agent process itself.

use std::time::Duration;

use crate::util::process::Child;
use agent_client_protocol::schema::v1::TerminalExitStatus;

/// Non-blocking poll for `child`'s exit status (`None` while still
/// running). Locks `child` only for the duration of the poll, so it never
/// contends with a concurrent kill/signal for longer than one syscall.
pub async fn current_exit_status(child: &smol::lock::Mutex<Child>) -> Option<TerminalExitStatus> {
    let mut child = child.lock().await;
    child.try_status().ok().flatten().map(exit_status_from)
}

/// Polls `child` every 25ms (matching acpx's own `waitMs(25)` cleanup loop)
/// until it exits, releasing the lock between polls so a concurrent
/// kill/signal call is never blocked behind this wait.
pub async fn poll_exit_status(child: &smol::lock::Mutex<Child>) -> TerminalExitStatus {
    loop {
        if let Some(status) = current_exit_status(child).await {
            return status;
        }
        smol::Timer::after(Duration::from_millis(25)).await;
    }
}

fn exit_status_from(status: std::process::ExitStatus) -> TerminalExitStatus {
    let exit_status = TerminalExitStatus::new().exit_code(status.code().map(|c| c as u32));
    #[cfg(unix)]
    let exit_status = {
        use std::os::unix::process::ExitStatusExt;
        exit_status.signal(status.signal().map(|s| s.to_string()))
    };
    exit_status
}

/// Portable signal numbers for [`send_group_signal`] callers that don't want
/// to depend on `libc` directly (it's a Unix-only target dependency in this
/// crate's `Cargo.toml`).
#[cfg(unix)]
pub const SIGTERM: i32 = libc::SIGTERM;
#[cfg(unix)]
pub const SIGKILL: i32 = libc::SIGKILL;
#[cfg(not(unix))]
pub const SIGTERM: i32 = 15;
#[cfg(not(unix))]
pub const SIGKILL: i32 = 9;

#[cfg(unix)]
pub fn has_live_process_group(pid: u32) -> bool {
    // SAFETY: signal 0 sends no signal, it only probes existence/permission;
    // negating `pid` targets the whole process group.
    unsafe { libc::kill(-(pid as libc::pid_t), 0) == 0 }
}

#[cfg(not(unix))]
pub fn has_live_process_group(_pid: u32) -> bool {
    false
}

/// Sends `signal` to `pid`'s entire process group. No-op on non-Unix
/// targets (see module docs) — callers fall back to
/// `crate::util::process::Child::kill` for the final SIGKILL step there.
#[cfg(unix)]
pub fn send_group_signal(pid: u32, signal: i32) {
    // SAFETY: signals a process group this crate itself spawned as the new
    // session leader (see module docs).
    unsafe {
        libc::killpg(pid as libc::pid_t, signal);
    }
}

#[cfg(not(unix))]
pub fn send_group_signal(_pid: u32, _signal: i32) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    #[test]
    fn group_signal_kills_own_process_group() {
        // `process_group(0)` makes the child its own group leader (pgid ==
        // pid), matching what `crate::util::process::Child::spawn`'s
        // `set_pre_exec_to_start_new_session` does for real terminal spawns.
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .process_group(0)
            .spawn()
            .expect("failed to spawn sleep");
        let pid = child.id();
        assert!(has_live_process_group(pid));

        send_group_signal(pid, libc::SIGKILL);
        child.wait().expect("wait for killed child");
        assert!(!has_live_process_group(pid));
    }
}
