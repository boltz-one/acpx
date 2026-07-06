//! Ports `others/acpx/src/spawn-command-options.ts`.
//!
//! acpx's Node `child_process.spawn` needs a `{ shell: true }` escape hatch
//! for Windows `.cmd`/`.bat` wrapper scripts (npm-installed CLIs are
//! frequently `.cmd` shims on Windows) because Node won't exec them
//! directly. `crate::util::command`/`crate::util::process` (this crate's spawn primitive
//! per ADR-3) has no such shell-wrapping concept, so [`should_use_windows_batch_shell`]
//! reports *whether* wrapping is needed and callers (currently
//! `terminal::spawn::spawn_terminal_process` via [`build_terminal_shell_spawn_command`],
//! and `client::spawn::spawn_agent_process` for the real agent-spawn path)
//! do the wrapping themselves at the `(program, args)` level instead of via
//! a spawn-option flag.
//!
//! The npm-specific executable *resolution* helper [`resolve_windows_command`]
//! (PATHEXT-based candidate resolution) is ported as a pure,
//! host-OS-independent function so it's testable on macOS/Linux CI; it is
//! only meaningful when actually spawning on Windows.
//!
//! `resolveWindowsExecutablePath`'s wrapper-script-content-sniffing half
//! (`resolveWindowsWrapperExecutable`/`resolveWindowsWrapperToken` — reading
//! a `.cmd`/`.bat` file's *contents* to find an embedded `.exe` target when
//! a naive PATHEXT lookup finds none) is **not** ported here — see ADR-10 in
//! `plans/20260706-0106-acp-completeness-fixes/phase-07-windows-batch-shell-spawn.md`.
//!
// TODO(gap-21b, gap-27): port others/acpx/src/spawn-command-options.ts's
// resolveWindowsWrapperExecutable/resolveWindowsWrapperToken and
// others/acpx/src/acp/agent-command.ts's resolveClaudeCodeExecutable once
// real Windows test infrastructure is available.

use std::path::{Path, PathBuf};

/// Ports `readWindowsEnvValue`: case-insensitive env lookup (Windows env
/// vars are case-insensitive; `std::env::var` on Windows already normalizes
/// this, but callers here operate on an explicit env map for testability).
pub fn read_windows_env_value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

fn windows_executable_extensions(env: &[(String, String)]) -> Vec<String> {
    read_windows_env_value(env, "PATHEXT")
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

fn command_candidates(command: &str, env: &[(String, String)]) -> Vec<String> {
    if Path::new(command).extension().is_some() {
        return vec![command.to_string()];
    }
    windows_executable_extensions(env)
        .into_iter()
        .map(|ext| format!("{command}{ext}"))
        .collect()
}

fn command_has_path(command: &str) -> bool {
    command.contains('/') || command.contains('\\') || Path::new(command).is_absolute()
}

/// Ports `resolveWindowsCommand`, given an `exists` predicate (injected so
/// this stays testable without a real filesystem/PATH).
pub fn resolve_windows_command(
    command: &str,
    env: &[(String, String)],
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let candidates = command_candidates(command, env);
    if command_has_path(command) {
        return candidates
            .into_iter()
            .map(PathBuf::from)
            .find(|c| exists(c));
    }
    let path_value = read_windows_env_value(env, "PATH")?;
    for dir in path_value.split(';') {
        let dir = dir.trim();
        if dir.is_empty() {
            continue;
        }
        if let Some(found) = candidates
            .iter()
            .map(|c| Path::new(dir).join(c))
            .find(|p| exists(p))
        {
            return Some(found);
        }
    }
    None
}

/// Ports `shouldUseWindowsBatchShell`: true when `command` resolves to a
/// `.cmd`/`.bat` file, which requires a shell wrapper to execute on Windows.
pub fn should_use_windows_batch_shell(
    command: &str,
    env: &[(String, String)],
    exists: impl Fn(&Path) -> bool,
) -> bool {
    let resolved = resolve_windows_command(command, env, exists);
    let ext = resolved
        .as_deref()
        .unwrap_or_else(|| Path::new(command))
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());
    matches!(ext.as_deref(), Some("cmd") | Some("bat"))
}

/// Ports `buildTerminalSpawnCommand`: run `command`/`args` directly, no
/// process-group kill needed (there's no shell to reap).
pub fn build_terminal_spawn_command(
    command: String,
    args: Vec<String>,
) -> (String, Vec<String>, bool) {
    (command, args, false)
}

/// Ports `buildTerminalShellSpawnCommand`: wrap `command` in a shell so
/// pipelines/redirection work, killing the whole process group on
/// terminate since a shell may spawn further children.
pub fn build_terminal_shell_spawn_command(
    command: &str,
    is_windows: bool,
) -> (String, Vec<String>, bool) {
    if is_windows {
        (
            "cmd.exe".to_string(),
            vec![
                "/d".to_string(),
                "/s".to_string(),
                "/c".to_string(),
                command.to_string(),
            ],
            true,
        )
    } else {
        (
            "/bin/sh".to_string(),
            vec!["-c".to_string(), command.to_string()],
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_windows_env_value_is_case_insensitive() {
        let env = vec![("PathExt".to_string(), ".EXE;.CMD".to_string())];
        assert_eq!(read_windows_env_value(&env, "PATHEXT"), Some(".EXE;.CMD"));
    }

    #[test]
    fn detects_batch_shell_by_extension() {
        let env: Vec<(String, String)> = vec![];
        assert!(should_use_windows_batch_shell(
            "C:/tools/claude.cmd",
            &env,
            |_| true
        ));
        assert!(!should_use_windows_batch_shell(
            "C:/tools/claude.exe",
            &env,
            |_| true
        ));
    }

    #[test]
    fn terminal_shell_spawn_uses_posix_sh_by_default() {
        let (command, args, kill_group) = build_terminal_shell_spawn_command("echo hi", false);
        assert_eq!(command, "/bin/sh");
        assert_eq!(args, vec!["-c", "echo hi"]);
        assert!(kill_group);
    }

    #[test]
    fn terminal_spawn_command_does_not_kill_group() {
        let (command, args, kill_group) =
            build_terminal_spawn_command("ls".to_string(), vec!["-la".to_string()]);
        assert_eq!(command, "ls");
        assert_eq!(args, vec!["-la"]);
        assert!(!kill_group);
    }
}
