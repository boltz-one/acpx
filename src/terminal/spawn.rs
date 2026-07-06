//! Terminal subprocess spawn. Ports the spawn half of
//! `others/acpx/src/acp/terminal-manager.ts` (`spawnTerminalProcess`,
//! `buildTerminalSpawnOptions`), reusing `agent_command::spawn_options`
//! (Phase 2) for the direct-vs-shell command decision and
//! `crate::util::command`/`crate::util::process` (ADR-3) for the actual spawn.
//!
//! Simplification vs acpx: the TS source always tries the direct command
//! first and only falls back to a shell wrapper after observing an `ENOENT`
//! spawn error (and only when the caller passed no separate `args`). This
//! port instead decides upfront using the same shell-syntax/whitespace
//! heuristic acpx's fallback path already uses, avoiding a wasted
//! spawn-then-retry round trip; behavior differs only for the rare case of
//! a program whose *filename* itself contains shell metacharacters, which
//! would need `args` supplied separately to spawn directly.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use crate::util::process::Child;
use agent_client_protocol::schema::v1::{CreateTerminalRequest, EnvVariable};

use crate::agent_command::spawn_options::{
    build_terminal_shell_spawn_command, build_terminal_spawn_command,
};
use crate::error::{AcpError, Result};

/// A spawned terminal command plus whether it should be killed as a whole
/// process group on `terminal/kill` (see `terminal::mod` docs: in this
/// port, every spawn already becomes its own process-group leader via
/// `crate::util::process::Child::spawn`, so this flag is currently informational).
pub struct SpawnedTerminal {
    pub child: Child,
    pub pid: u32,
    pub kill_process_group: bool,
}

const SHELL_METACHARACTERS: &[char] = &[
    '|', '&', ';', '<', '>', '(', ')', '$', '`', '*', '?', '[', ']', '{', '}', '\'', '"', '\\',
    '\r', '\n',
];

fn needs_shell_wrap(command: &str, args_absent: bool) -> bool {
    args_absent && (command.contains(SHELL_METACHARACTERS) || command.contains(char::is_whitespace))
}

fn terminal_env(entries: &[EnvVariable]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|entry| (entry.name.clone(), entry.value.clone()))
        .collect()
}

/// Spawns `params.command` (optionally shell-wrapped), matching acpx's
/// `stdio: ["ignore", "pipe", "pipe"]` and full-environment-inherit +
/// override semantics (unlike `agent_command::spawn`'s fully-controlled
/// environment, a terminal command inherits this process's environment and
/// only overrides the entries the caller specified).
pub fn spawn_terminal_process(
    params: &CreateTerminalRequest,
    default_cwd: &Path,
) -> Result<SpawnedTerminal> {
    let args_absent = params.args.is_empty();
    let (program, args, kill_process_group) = if needs_shell_wrap(&params.command, args_absent) {
        build_terminal_shell_spawn_command(&params.command, cfg!(windows))
    } else {
        build_terminal_spawn_command(params.command.clone(), params.args.clone())
    };

    let cwd = params.cwd.as_deref().unwrap_or(default_cwd);
    let env = terminal_env(&params.env);

    let mut command = crate::util::command::new_std_command(&program);
    command.args(&args);
    command.current_dir(cwd);
    if !env.is_empty() {
        command.envs(&env);
    }

    let child =
        Child::spawn(command, Stdio::null(), Stdio::piped(), Stdio::piped()).map_err(|source| {
            AcpError::TerminalSpawn {
                command: display_command(&program, &args),
                source: source
                    .downcast::<std::io::Error>()
                    .unwrap_or_else(|err| std::io::Error::other(err.to_string())),
            }
        })?;
    let pid = child.id();

    Ok(SpawnedTerminal {
        child,
        pid,
        kill_process_group,
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
    fn direct_command_with_args_does_not_shell_wrap() {
        assert!(!needs_shell_wrap("/bin/echo", false));
    }

    #[test]
    fn bare_command_with_shell_syntax_and_no_args_wraps() {
        assert!(needs_shell_wrap("echo hi && echo bye", true));
    }

    #[test]
    fn simple_command_without_args_does_not_wrap() {
        assert!(!needs_shell_wrap("ls", true));
    }

    #[test]
    fn spawns_and_captures_stdio() {
        smol::block_on(async {
            let params = CreateTerminalRequest::new("s1", "echo").args(vec!["hello".to_string()]);
            let spawned =
                spawn_terminal_process(&params, Path::new("/tmp")).expect("spawn should succeed");
            assert!(spawned.pid > 0);
            let output = spawned.child.into_inner().output().await.expect("output");
            assert!(output.status.success());
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
        });
    }

    #[test]
    fn shell_syntax_command_line_is_wrapped_and_runs() {
        smol::block_on(async {
            let params = CreateTerminalRequest::new("s1", "echo one && echo two");
            let spawned =
                spawn_terminal_process(&params, Path::new("/tmp")).expect("spawn should succeed");
            assert!(spawned.kill_process_group);
            let output = spawned.child.into_inner().output().await.expect("output");
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "one\ntwo");
        });
    }

    #[test]
    fn missing_program_returns_terminal_spawn_error() {
        let params = CreateTerminalRequest::new("s1", "/definitely/not/a/real/binary-xyz");
        let result = spawn_terminal_process(&params, Path::new("/tmp"));
        assert!(matches!(result, Err(AcpError::TerminalSpawn { .. })));
    }
}
