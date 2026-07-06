//! SIGTERM(1.5s)->SIGKILL(1s) grace-period shutdown. Ports the shutdown half
//! of `others/acpx/src/acp/client-process.ts`.
//!
//! `crate::util::process::Child::kill` (per ADR-3) only sends SIGKILL
//! (`libc::killpg(pid, SIGKILL)`) — there is no signal-specific variant to
//! reuse for the initial graceful SIGTERM. Rather than extend `util` (a
//! near-universal workspace dependency) for one caller, this module sends
//! SIGTERM directly via `libc::killpg`, matching `util`'s own
//! whole-process-group semantics, then escalates to `crate::util::process::Child::kill`
//! for the SIGKILL step so the final kill path stays shared.

use std::time::Duration;

use crate::util::process::Child;
use smol::Timer;

use crate::agent_command::command_args::resolve_agent_close_after_stdin_end_ms;
use crate::client::state::AgentExitInfo;
use crate::control::with_timeout;
use crate::platform::is_process_alive;

const SIGTERM_GRACE: Duration = Duration::from_millis(1500);
const SIGKILL_GRACE: Duration = Duration::from_secs(1);

/// Closes `child`'s stdin, sends SIGTERM to its process group, waits up to
/// [`SIGTERM_GRACE`] for it to exit, escalates to SIGKILL via `util`'s
/// [`Child::kill`] and waits up to [`SIGKILL_GRACE`], then reports how it
/// went down. Never returns an error: a shutdown sequence's job is to
/// guarantee the process is gone, not to propagate why waiting failed.
///
/// Closes stdin with no extra grace period before SIGTERM (matching this
/// function's pre-gap-22 behavior), for callers that don't know the
/// resolved `agentCommand` string of the process being shut down. Prefer
/// [`shutdown_agent_process_for_agent_command`] when it's available: it
/// applies Qoder's longer stdin-close-to-SIGTERM grace period
/// (`resolve_agent_close_after_stdin_end_ms`, gap 22) for agent commands
/// that need it. This crate's one current caller
/// ([`crate::client::AcpClient::shutdown`]) doesn't carry a resolved
/// `agentCommand` string on `self` today (see that struct's doc comment),
/// so it can't be switched to the command-aware variant without adding a
/// field to `AcpClient` — out of this phase's `client/shutdown.rs`-only
/// file-ownership scope.
pub async fn shutdown_agent_process(child: &mut Child, pid: u32) -> AgentExitInfo {
    shutdown_agent_process_after_stdin_delay(child, pid, Duration::ZERO).await
}

/// Like [`shutdown_agent_process`], but resolves the stdin-close-to-SIGTERM
/// delay from `agent_command` (the resolved command line the agent process
/// was spawned with) via [`resolve_agent_close_after_stdin_end_ms`]. Ports
/// the delay half of acpx's Qoder-specific shutdown handling (gap 22).
pub async fn shutdown_agent_process_for_agent_command(
    child: &mut Child,
    pid: u32,
    agent_command: &str,
) -> AgentExitInfo {
    let delay_ms = resolve_agent_close_after_stdin_end_ms(agent_command).unwrap_or(0);
    shutdown_agent_process_after_stdin_delay(child, pid, Duration::from_millis(delay_ms)).await
}

async fn shutdown_agent_process_after_stdin_delay(
    child: &mut Child,
    pid: u32,
    stdin_close_delay: Duration,
) -> AgentExitInfo {
    drop(child.stdin.take());
    if stdin_close_delay > Duration::ZERO {
        Timer::after(stdin_close_delay).await;
    }

    // SAFETY: signals a process group this crate itself spawned (per
    // `crate::util::process::Child::spawn`'s `set_pre_exec_to_start_new_session`).
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGTERM);
    }
    if with_timeout(wait_for_exit(child), Some(SIGTERM_GRACE))
        .await
        .is_ok()
    {
        return exit_info("sigterm", child).await;
    }

    if let Err(err) = child.kill() {
        log::warn!("failed to SIGKILL agent process {pid}: {err}");
    }
    if with_timeout(wait_for_exit(child), Some(SIGKILL_GRACE))
        .await
        .is_ok()
    {
        return exit_info("sigkill", child).await;
    }

    AgentExitInfo {
        reason: "sigkill_timeout".to_string(),
        exit_code: None,
        signal: is_process_alive(pid).then(|| "unknown".to_string()),
    }
}

async fn wait_for_exit(child: &mut Child) {
    let _ = child.status().await;
}

async fn exit_info(reason: &str, child: &mut Child) -> AgentExitInfo {
    let status = child.status().await.ok();
    AgentExitInfo {
        reason: reason.to_string(),
        exit_code: status.and_then(|s| s.code()),
        signal: signal_from_status(status),
    }
}

#[cfg(unix)]
fn signal_from_status(status: Option<std::process::ExitStatus>) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    status.and_then(|s| s.signal()).map(|s| s.to_string())
}

#[cfg(not(unix))]
fn signal_from_status(_status: Option<std::process::ExitStatus>) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::spawn::{SpawnOptions, spawn_agent_process};
    use std::collections::HashMap;
    use std::path::Path;

    #[test]
    fn graceful_process_exits_on_sigterm() {
        smol::block_on(async {
            let env = HashMap::new();
            let mut child = spawn_agent_process(SpawnOptions {
                program: "/bin/sleep",
                args: &["30".to_string()],
                cwd: Path::new("/tmp"),
                env: &env,
            })
            .expect("spawn should succeed");
            let pid = child.id();

            let info = shutdown_agent_process(&mut child, pid).await;
            assert_eq!(info.reason, "sigterm");
            assert!(!is_process_alive(pid));
        });
    }

    #[test]
    fn qoder_detected_command_waits_configured_delay_before_sigterm() {
        smol::block_on(async {
            let env = HashMap::new();
            let mut child = spawn_agent_process(SpawnOptions {
                program: "/bin/sleep",
                args: &["30".to_string()],
                cwd: Path::new("/tmp"),
                env: &env,
            })
            .expect("spawn should succeed");
            let pid = child.id();

            let start = std::time::Instant::now();
            let info =
                shutdown_agent_process_for_agent_command(&mut child, pid, "qodercli --acp").await;
            let elapsed = start.elapsed();

            assert_eq!(info.reason, "sigterm");
            assert!(
                elapsed >= Duration::from_millis(750),
                "expected at least Qoder's stdin-close delay before SIGTERM, got {elapsed:?}"
            );
        });
    }

    #[test]
    fn non_qoder_command_has_no_qoder_length_delay() {
        smol::block_on(async {
            let env = HashMap::new();
            let mut child = spawn_agent_process(SpawnOptions {
                program: "/bin/sleep",
                args: &["30".to_string()],
                cwd: Path::new("/tmp"),
                env: &env,
            })
            .expect("spawn should succeed");
            let pid = child.id();

            let start = std::time::Instant::now();
            let info =
                shutdown_agent_process_for_agent_command(&mut child, pid, "cursor-agent acp").await;
            let elapsed = start.elapsed();

            assert_eq!(info.reason, "sigterm");
            assert!(
                elapsed < Duration::from_millis(750),
                "expected no Qoder-length delay for a non-Qoder command, got {elapsed:?}"
            );
        });
    }
}
