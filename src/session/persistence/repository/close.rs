//! Ports `closeSession`: best-effort terminate the recorded pid, mark the
//! record closed, and persist it.

use crate::error::Result;
use crate::session::conversation_model::iso_now;
use crate::session::record::SessionRecord;
use crate::session::store_options::AcpFileSessionStoreOptions;

use super::resolve::resolve_session_record;
use super::write::write_session_record;
use crate::session::persistence::index::rebuild_session_index;

/// Maps a recorded exit-signal string to a raw signal number, for the
/// "resend whatever signal last killed it" candidate in
/// [`kill_signal_candidates`]. Accepts canonical `SIG*` names (matching
/// Node's `NodeJS.Signals`, what acpx's `lastAgentExitSignal` is populated
/// with) case-insensitively, with or without the `SIG` prefix, as well as a
/// plain signal number (this crate's own exit-signal recording path,
/// `client/shutdown.rs::signal_from_status`, stores the numeric form).
#[cfg(unix)]
fn signal_number_from_name(name: &str) -> Option<libc::c_int> {
    if let Ok(number) = name.trim().parse::<libc::c_int>() {
        return Some(number);
    }
    let normalized = name.trim().to_uppercase();
    let normalized = normalized.strip_prefix("SIG").unwrap_or(&normalized);
    Some(match normalized {
        "TERM" => libc::SIGTERM,
        "KILL" => libc::SIGKILL,
        "INT" => libc::SIGINT,
        "HUP" => libc::SIGHUP,
        "QUIT" => libc::SIGQUIT,
        "ABRT" => libc::SIGABRT,
        "ALRM" => libc::SIGALRM,
        "USR1" => libc::SIGUSR1,
        "USR2" => libc::SIGUSR2,
        "CONT" => libc::SIGCONT,
        "STOP" => libc::SIGSTOP,
        "PIPE" => libc::SIGPIPE,
        _ => return None,
    })
}

/// Ports acpx's `killSignalCandidates`: `None` (never recorded an exit
/// signal) escalates through SIGTERM then SIGKILL as before; a previously
/// recorded `SIGKILL` skips straight to SIGKILL (no point re-sending a
/// signal that already failed to bring the process down cleanly); any other
/// recorded signal is resent once, then SIGKILL.
#[cfg(unix)]
fn kill_signal_candidates(last_agent_exit_signal: Option<&str>) -> Vec<libc::c_int> {
    let Some(signal) = last_agent_exit_signal else {
        return vec![libc::SIGTERM, libc::SIGKILL];
    };
    let normalized = signal.trim().to_uppercase();
    if normalized == "SIGKILL" || normalized == "KILL" {
        return vec![libc::SIGKILL];
    }
    let mut candidates = Vec::new();
    if let Some(number) = signal_number_from_name(signal) {
        candidates.push(number);
    }
    candidates.push(libc::SIGKILL);
    candidates
}

#[cfg(unix)]
fn best_effort_terminate(pid: u32, last_agent_exit_signal: Option<&str>) {
    // SAFETY: sends standard termination signals to a pid recorded in a
    // session file. Not necessarily a child of this process (the agent may
    // have outlived a previous run of this crate's host), so this uses
    // `libc::kill` (single pid) rather than `libc::killpg` (process group) —
    // ports acpx's plain `process.kill(pid, signal)` calls in `closeSession`.
    //
    // Each signal is sent independently and its result ignored (matching
    // acpx's per-signal `try { process.kill(...) } catch {}`): a failure to
    // deliver one candidate signal (e.g. the pid is already gone) shouldn't
    // stop the remaining candidates from being attempted.
    for signal in kill_signal_candidates(last_agent_exit_signal) {
        unsafe {
            libc::kill(pid as libc::pid_t, signal);
        }
    }
}

#[cfg(windows)]
fn best_effort_terminate(pid: u32, _last_agent_exit_signal: Option<&str>) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(handle, 1);
            let _ = CloseHandle(handle);
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn best_effort_terminate(_pid: u32, _last_agent_exit_signal: Option<&str>) {}

/// Ports `closeSession`.
pub fn close_session(options: &AcpFileSessionStoreOptions, id: &str) -> Result<SessionRecord> {
    let mut record = resolve_session_record(options, id)?;
    let now = iso_now();

    if let Some(pid) = record.pid {
        best_effort_terminate(pid, record.last_agent_exit_signal.as_deref());
    }

    record.closed = true;
    record.closed_at = Some(now.clone());
    record.pid = None;
    record.last_used_at = now.clone();
    record.last_prompt_at.get_or_insert(now);

    write_session_record(options, &record)?;
    let _ = rebuild_session_index(options);
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    fn options(dir: &tempfile::TempDir) -> AcpFileSessionStoreOptions {
        AcpFileSessionStoreOptions::new(dir.path())
    }

    #[test]
    fn close_session_marks_closed_and_clears_pid() {
        let dir = tempfile::tempdir().unwrap();
        let options = options(&dir);
        let record = sample_session_record();
        write_session_record(&options, &record).unwrap();

        let closed = close_session(&options, &record.acpx_record_id).unwrap();
        assert!(closed.closed);
        assert!(closed.pid.is_none());
    }

    #[test]
    #[cfg(unix)]
    fn kill_signal_candidates_defaults_to_sigterm_then_sigkill() {
        assert_eq!(
            kill_signal_candidates(None),
            vec![libc::SIGTERM, libc::SIGKILL]
        );
    }

    #[test]
    #[cfg(unix)]
    fn kill_signal_candidates_skips_sigterm_when_last_exit_was_sigkill() {
        assert_eq!(kill_signal_candidates(Some("SIGKILL")), vec![libc::SIGKILL]);
        // Case-insensitive, matching acpx's `signal.toUpperCase()`.
        assert_eq!(kill_signal_candidates(Some("sigkill")), vec![libc::SIGKILL]);
    }

    #[test]
    #[cfg(unix)]
    fn kill_signal_candidates_resends_other_recorded_signal_then_sigkill() {
        assert_eq!(
            kill_signal_candidates(Some("SIGHUP")),
            vec![libc::SIGHUP, libc::SIGKILL]
        );
        // This crate's own `signal_from_status` records numeric signal
        // strings (e.g. "15" for SIGTERM) rather than `SIG*` names.
        assert_eq!(kill_signal_candidates(Some("15")), vec![15, libc::SIGKILL]);
    }

    #[test]
    #[cfg(unix)]
    fn close_session_terminates_a_live_recorded_pid() {
        let dir = tempfile::tempdir().unwrap();
        let options = options(&dir);
        let mut record = sample_session_record();

        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("failed to spawn sleep");
        record.pid = Some(child.id());
        write_session_record(&options, &record).unwrap();

        let closed = close_session(&options, &record.acpx_record_id).unwrap();
        assert!(closed.pid.is_none());

        std::thread::sleep(std::time::Duration::from_millis(200));
        let _ = child.try_wait();
        assert!(!crate::platform::is_process_alive(child.id()));
    }
}
