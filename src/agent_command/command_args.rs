//! Ports the command-line splitting/quoting from
//! `others/acpx/src/acp/client-process.ts` (`splitCommandLine`,
//! `basenameToken`) and the per-agent CLI detection predicates from
//! `others/acpx/src/acp/agent-command.ts`.
//!
//! [`split_command_line`] is a direct character-by-character port of acpx's
//! hand-rolled state machine, not a call into a general shell-lexer crate:
//! per the phase's Security Considerations, a persisted `agentCommand`
//! string is untrusted config, and even a semantically-close-but-different
//! quoting/escaping implementation could let arguments swallow flags they
//! shouldn't (or vice versa) when acpx and this port disagree on an edge
//! case (e.g. backslash handling inside single quotes).

use crate::error::{AcpError, Result};

/// A resolved `(command, args)` pair. Ports acpx's `CommandParts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandParts {
    pub command: String,
    pub args: Vec<String>,
}

/// Ports `splitCommandLine`: POSIX-shell-like tokenizing with `'`/`"` quoting
/// and `\` escaping (backslash has no special meaning inside single quotes,
/// matching POSIX/acpx's behavior).
pub fn split_command_line(value: &str) -> Result<CommandParts> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaping = false;
    let mut has_part = false;

    for ch in value.chars() {
        if escaping {
            current.push(ch);
            escaping = false;
            has_part = true;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaping = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                current.push(ch);
            }
            has_part = true;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            has_part = true;
            continue;
        }
        if ch.is_whitespace() {
            if has_part {
                parts.push(std::mem::take(&mut current));
            }
            has_part = false;
            continue;
        }
        current.push(ch);
        has_part = true;
    }

    if escaping {
        current.push('\\');
        has_part = true;
    }
    if quote.is_some() {
        return Err(AcpError::Other(anyhow::anyhow!(
            "Invalid --agent command: unterminated quote"
        )));
    }
    if has_part {
        parts.push(current);
    }
    if parts.is_empty() || parts[0].is_empty() {
        return Err(AcpError::Other(anyhow::anyhow!(
            "Invalid --agent command: empty command"
        )));
    }

    let command = parts.remove(0);
    Ok(CommandParts {
        command,
        args: parts,
    })
}

/// Ports `basenameToken`: lowercased basename with a trailing
/// `.cmd`/`.exe`/`.bat` extension stripped, used to identify a command
/// regardless of platform executable suffix or invocation path.
pub fn basename_token(value: &str) -> String {
    let base = value.rsplit(['/', '\\']).next().unwrap_or(value);
    let lower = base.to_lowercase();
    for ext in [".cmd", ".exe", ".bat"] {
        if let Some(stripped) = lower.strip_suffix(ext) {
            return stripped.to_string();
        }
    }
    lower
}

const DEFAULT_AGENT_CLOSE_AFTER_STDIN_END_MS: u64 = 100;
const QODER_AGENT_CLOSE_AFTER_STDIN_END_MS: u64 = 750;

/// Ports `resolveAgentCloseAfterStdinEndMs`.
pub fn resolve_agent_close_after_stdin_end_ms(agent_command: &str) -> Result<u64> {
    let parts = split_command_line(agent_command)?;
    Ok(if basename_token(&parts.command) == "qodercli" {
        QODER_AGENT_CLOSE_AFTER_STDIN_END_MS
    } else {
        DEFAULT_AGENT_CLOSE_AFTER_STDIN_END_MS
    })
}

const QODER_BENIGN_STDOUT_LINES: [&str; 2] = [
    "Received interrupt signal. Cleaning up resources...",
    "Cleanup completed. Exiting...",
];

/// Ports `shouldIgnoreNonJsonAgentOutputLine`.
///
/// TODO(gap-22): this predicate's natural call site is wherever non-JSON
/// agent stdout lines are logged as warnings during the transport read loop
/// (`client/transport.rs`'s `filter_map`/log-and-drop path for lines that
/// fail JSON-RPC parsing), which is out of `phase-08`'s declared file scope
/// (`client/shutdown.rs`, `agent_command/{command_args,registry}.rs`,
/// `session/persistence/repository/{close,prune}.rs`). Deferred rather than
/// silently dropped — wiring this in is a small, well-scoped follow-up: at
/// that call site, skip the warning log (but still discard the line) when
/// `should_ignore_non_json_agent_output_line(agent_command, trimmed_line)`
/// is `true`.
pub fn should_ignore_non_json_agent_output_line(agent_command: &str, trimmed_line: &str) -> bool {
    let Ok(parts) = split_command_line(agent_command) else {
        return false;
    };
    basename_token(&parts.command) == "qodercli"
        && QODER_BENIGN_STDOUT_LINES.contains(&trimmed_line)
}

// `buildQoderAcpCommandArgs` (acpx's Qoder-specific `--max-turns`/
// `--allowed-tools` CLI-arg injection, `others/acpx/src/acp/agent-command.ts`
// L98-119) is deliberately NOT ported here. Per plan.md's Unresolved
// Questions #7 and this phase's Locked-in Decisions, it's confirmed
// deferred: it mutates the resolved argv for a single agent's CLI surface
// (not a shutdown/detection primitive like the other three gap-22/26
// functions), and no current caller in this crate resolves per-agent
// `maxTurns`/`allowedTools` session options into CLI args at all yet — that
// plumbing would need to land first, in a future phase, before this
// function has anywhere real to be wired.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_simple_command() {
        let parts = split_command_line("cursor-agent acp").unwrap();
        assert_eq!(parts.command, "cursor-agent");
        assert_eq!(parts.args, vec!["acp"]);
    }

    #[test]
    fn respects_double_quotes_with_spaces() {
        let parts = split_command_line(r#"node "my agent.js" --flag"#).unwrap();
        assert_eq!(parts.command, "node");
        assert_eq!(parts.args, vec!["my agent.js", "--flag"]);
    }

    #[test]
    fn single_quotes_disable_backslash_escaping() {
        let parts = split_command_line(r"echo 'a\b'").unwrap();
        assert_eq!(parts.args, vec![r"a\b"]);
    }

    #[test]
    fn backslash_escapes_next_char_outside_single_quotes() {
        let parts = split_command_line(r#"echo a\ b"#).unwrap();
        assert_eq!(parts.args, vec!["a b"]);
    }

    #[test]
    fn unterminated_quote_is_an_error() {
        assert!(split_command_line("echo \"unterminated").is_err());
    }

    #[test]
    fn empty_command_is_an_error() {
        assert!(split_command_line("   ").is_err());
    }

    #[test]
    fn basename_token_strips_extension_and_path() {
        assert_eq!(
            basename_token("/usr/local/bin/Claude-Agent-ACP.CMD"),
            "claude-agent-acp"
        );
    }

    #[test]
    fn qoder_command_gets_longer_stdin_close_delay() {
        assert_eq!(
            resolve_agent_close_after_stdin_end_ms("qodercli --acp").unwrap(),
            QODER_AGENT_CLOSE_AFTER_STDIN_END_MS
        );
        assert_eq!(
            resolve_agent_close_after_stdin_end_ms("/usr/local/bin/QoderCLI.exe --acp").unwrap(),
            QODER_AGENT_CLOSE_AFTER_STDIN_END_MS
        );
    }

    #[test]
    fn non_qoder_command_gets_default_stdin_close_delay() {
        assert_eq!(
            resolve_agent_close_after_stdin_end_ms("cursor-agent acp").unwrap(),
            DEFAULT_AGENT_CLOSE_AFTER_STDIN_END_MS
        );
    }

    #[test]
    fn qoder_benign_line_is_ignored() {
        assert!(should_ignore_non_json_agent_output_line(
            "qodercli --acp",
            "Received interrupt signal. Cleaning up resources..."
        ));
    }

    #[test]
    fn non_qoder_command_never_ignores_lines() {
        assert!(!should_ignore_non_json_agent_output_line(
            "cursor-agent acp",
            "Received interrupt signal. Cleaning up resources..."
        ));
    }

    #[test]
    fn qoder_command_only_ignores_known_benign_lines() {
        assert!(!should_ignore_non_json_agent_output_line(
            "qodercli --acp",
            "some other stdout line"
        ));
    }
}
