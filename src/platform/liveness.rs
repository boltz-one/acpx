//! Ports `others/acpx/src/process-liveness.ts`'s `isProcessAlive`.

/// Returns `true` if `pid` refers to a live process. Never returns `true`
/// for `pid == 0` or the current process's own pid, matching acpx's guards.
#[cfg(unix)]
pub fn is_process_alive(pid: u32) -> bool {
    if pid == 0 || pid == std::process::id() {
        return false;
    }
    // SAFETY: signal 0 sends no signal; it only checks existence/permission.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(windows)]
pub fn is_process_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    const STILL_ACTIVE: u32 = 259;

    if pid == 0 || pid == std::process::id() {
        return false;
    }

    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut exit_code = 0u32;
        let alive = GetExitCodeProcess(handle, &mut exit_code).is_ok() && exit_code == STILL_ACTIVE;
        let _ = CloseHandle(handle);
        alive
    }
}

#[cfg(not(any(unix, windows)))]
pub fn is_process_alive(_pid: u32) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_not_reported_alive() {
        assert!(!is_process_alive(std::process::id()));
    }

    #[test]
    fn zero_pid_is_never_alive() {
        assert!(!is_process_alive(0));
    }

    #[test]
    fn spawned_child_is_alive_until_reaped() {
        // A signal-permission check (EPERM vs ESRCH) is conflated into a
        // single `false` by this function, matching acpx's own
        // `process.kill(pid, 0)` behavior — so pid 1 isn't a reliable test
        // target under a sandboxed test runner. A self-spawned child always
        // has signal permission from this process.
        let mut child = std::process::Command::new("sleep")
            .arg("1")
            .spawn()
            .expect("failed to spawn sleep");
        assert!(is_process_alive(child.id()));
        child.wait().expect("failed to wait for child");
        assert!(!is_process_alive(child.id()));
    }
}
