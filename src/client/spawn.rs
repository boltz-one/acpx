//! Subprocess spawn for the ACP agent. Ports the spawn half of
//! `others/acpx/src/acp/client-process.ts` (`waitForSpawn`,
//! `requireAgentStdio`), reusing `crate::util::command`/`crate::util::process` per ADR-3
//! instead of re-deriving spawn/kill primitives.
//!
//! Also ports the real agent-spawn call site's use of acpx's
//! `buildSpawnCommandOptions` (`others/acpx/src/spawn-command-options.ts`,
//! wired in at `others/acpx/src/acp/agent-command.ts:205`): when the
//! resolved `program` is a Windows `.cmd`/`.bat` shim, the spawn is routed
//! through a shell (see [`spawn_agent_process_with`]) instead of exec'd
//! directly. See Phase 7
//! (`plans/20260706-0106-acp-completeness-fixes/phase-07-windows-batch-shell-spawn.md`).

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use crate::util::process::Child;

use crate::agent_command::spawn_options::{
    build_terminal_shell_spawn_command, should_use_windows_batch_shell,
};
use crate::error::{AcpError, Result};

/// Everything needed to spawn one ACP agent process.
pub struct SpawnOptions<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub cwd: &'a Path,
    /// Full environment for the child (already merged via
    /// [`crate::auth_env::build_agent_environment`] by the caller).
    pub env: &'a HashMap<String, String>,
}

/// Spawns the agent with piped stdin/stdout/stderr, matching acpx's
/// `buildAgentSpawnOptions` (`stdio: ["pipe", "pipe", "pipe"]`,
/// `windowsHide: true` — the latter is `crate::util::command::new_std_command`'s
/// `CREATE_NO_WINDOW` flag on Windows).
///
/// Thin wrapper over [`spawn_agent_process_with`] that defaults
/// `is_windows` to `cfg!(windows)` and `exists` to a real filesystem check.
/// Kept as a stable, non-parameterized entry point since this is called
/// from `client/mod.rs::AcpClient::spawn` (out of this phase's scope);
/// `spawn_agent_process_with` is the dependency-injected helper unit tests
/// exercise directly.
pub fn spawn_agent_process(options: SpawnOptions<'_>) -> Result<Child> {
    spawn_agent_process_with(options, cfg!(windows), |path| path.exists())
}

/// Ports the real agent-spawn call site's use of acpx's
/// `buildSpawnCommandOptions`/`shouldUseWindowsBatchShell`
/// (`others/acpx/src/spawn-command-options.ts`, wired in at
/// `others/acpx/src/acp/agent-command.ts:205`): when `options.program`
/// resolves to a Windows `.cmd`/`.bat` shim, the spawn is routed through
/// `cmd.exe /d /s /c "<program> <args...>"` (via
/// [`build_terminal_shell_spawn_command`], the same helper already used by
/// `terminal::spawn::spawn_terminal_process` — no second quoting scheme is
/// invented here) instead of exec'ing the shim directly, which otherwise
/// fails to spawn on Windows outright.
///
/// `program`+`args` are joined with a single space before being handed to
/// the shell, mirroring Node's own `shell: true` behavior for
/// `child_process.spawn(file, args, { shell: true })` (Node itself just
/// space-joins `[file, ...args]` into one command line for `cmd.exe` to
/// re-parse — no per-argument quoting is added on top of that by acpx
/// either, so this port doesn't add any either).
///
/// `is_windows`/`exists` are dependency-injected (matching
/// [`should_use_windows_batch_shell`]'s own existing pattern) so this is
/// unit-testable on non-Windows CI. Real Windows-native validation of the
/// resulting `cmd.exe` invocation (e.g. real-world argument-quoting edge
/// cases) is deferred — no Windows CI is available in this environment;
/// see Phase 7's Risk Assessment
/// (`plans/20260706-0106-acp-completeness-fixes/phase-07-windows-batch-shell-spawn.md`).
fn spawn_agent_process_with(
    options: SpawnOptions<'_>,
    is_windows: bool,
    exists: impl Fn(&Path) -> bool,
) -> Result<Child> {
    let env_pairs: Vec<(String, String)> = options
        .env
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    let (program, args) =
        if is_windows && should_use_windows_batch_shell(options.program, &env_pairs, exists) {
            let joined_command = std::iter::once(options.program.to_string())
                .chain(options.args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");
            let (shell_program, shell_args, _kill_process_group) =
                build_terminal_shell_spawn_command(&joined_command, is_windows);
            (shell_program, shell_args)
        } else {
            (options.program.to_string(), options.args.to_vec())
        };

    let mut command = crate::util::command::new_std_command(&program);
    command.args(&args);
    command.current_dir(options.cwd);
    command.env_clear();
    command.envs(options.env);

    Child::spawn(command, Stdio::piped(), Stdio::piped(), Stdio::piped()).map_err(|source| {
        AcpError::AgentSpawn {
            command: display_command(&program, &args),
            source: source
                .downcast::<std::io::Error>()
                .unwrap_or_else(|err| std::io::Error::other(err.to_string())),
        }
    })
}

fn display_command(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawns_and_captures_stdio() {
        smol::block_on(async {
            let env = HashMap::from([(
                "PATH".to_string(),
                std::env::var("PATH").unwrap_or_default(),
            )]);
            let child = spawn_agent_process(SpawnOptions {
                program: "/bin/echo",
                args: &["hello".to_string()],
                cwd: Path::new("/tmp"),
                env: &env,
            })
            .expect("spawn should succeed");

            assert!(child.stdout.is_some());
            let output = child.into_inner().output().await.expect("output");
            assert!(output.status.success());
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
        });
    }

    #[test]
    fn missing_program_returns_agent_spawn_error() {
        let env = HashMap::new();
        let result = spawn_agent_process(SpawnOptions {
            program: "/definitely/not/a/real/binary-xyz",
            args: &[],
            cwd: Path::new("/tmp"),
            env: &env,
        });
        assert!(matches!(result, Err(AcpError::AgentSpawn { .. })));
    }

    // The following tests force `is_windows`/`exists` via
    // `spawn_agent_process_with` (dependency injection, not
    // `cfg!(windows)`), mirroring `agent_command::spawn_options`'s own
    // `should_use_windows_batch_shell` test pattern. Since this CI host
    // isn't Windows, the actual `Child::spawn` call always fails here
    // (`cmd.exe` isn't a real binary on this host) — what these tests prove
    // is that `spawn_agent_process_with`'s *real* code path builds a
    // shell-wrapped (or unwrapped) invocation, via the `command` field of
    // the resulting `AcpError::AgentSpawn`, not just that the pure
    // `should_use_windows_batch_shell` predicate returns the right bool in
    // isolation. True Windows-native validation (real `cmd.exe` argument
    // parsing) remains deferred — no Windows CI is available here.

    #[test]
    fn windows_cmd_shim_is_shell_wrapped_when_is_windows_true() {
        let env = HashMap::new();
        let result = spawn_agent_process_with(
            SpawnOptions {
                program: "claude.cmd",
                args: &["--acp".to_string()],
                cwd: Path::new("/tmp"),
                env: &env,
            },
            true,
            |_| true,
        );

        match result {
            Err(AcpError::AgentSpawn { command, .. }) => {
                assert_eq!(command, r#"cmd.exe /d /s /c claude.cmd --acp"#);
            }
            Ok(_) => panic!("expected AgentSpawn error, spawn unexpectedly succeeded"),
            Err(other) => panic!("expected AgentSpawn error, got a different variant: {other}"),
        }
    }

    #[test]
    fn non_windows_cmd_shim_is_not_shell_wrapped() {
        let env = HashMap::new();
        let result = spawn_agent_process_with(
            SpawnOptions {
                program: "/definitely/not/a/real/binary.cmd",
                args: &[],
                cwd: Path::new("/tmp"),
                env: &env,
            },
            false,
            |_| true,
        );

        match result {
            Err(AcpError::AgentSpawn { command, .. }) => {
                assert_eq!(command, "/definitely/not/a/real/binary.cmd");
            }
            Ok(_) => panic!("expected AgentSpawn error, spawn unexpectedly succeeded"),
            Err(other) => panic!("expected AgentSpawn error, got a different variant: {other}"),
        }
    }

    #[test]
    fn windows_exe_is_not_shell_wrapped() {
        let env = HashMap::new();
        let result = spawn_agent_process_with(
            SpawnOptions {
                program: "claude.exe",
                args: &[],
                cwd: Path::new("/tmp"),
                env: &env,
            },
            true,
            |_| true,
        );

        match result {
            Err(AcpError::AgentSpawn { command, .. }) => {
                assert_eq!(command, "claude.exe");
            }
            Ok(_) => panic!("expected AgentSpawn error, spawn unexpectedly succeeded"),
            Err(other) => panic!("expected AgentSpawn error, got a different variant: {other}"),
        }
    }
}
