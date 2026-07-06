//! Per-agent CLI detection predicates ported from
//! `others/acpx/src/acp/agent-command.ts`. Split out of `command_args.rs`
//! (which owns generic command-line splitting, not per-agent quirks) to
//! keep both files under the 200-line convention.

use super::command_args::basename_token;

/// Ports `isGeminiAcpCommand`.
pub fn is_gemini_acp_command(command: &str, args: &[String]) -> bool {
    basename_token(command) == "gemini"
        && (args.iter().any(|a| a == "--acp") || args.iter().any(|a| a == "--experimental-acp"))
}

/// Ports `isClaudeAcpCommand`.
pub fn is_claude_acp_command(command: &str, args: &[String]) -> bool {
    basename_token(command) == "claude-agent-acp"
        || args.iter().any(|a| a.contains("claude-agent-acp"))
}

/// Ports `isCopilotAcpCommand`.
pub fn is_copilot_acp_command(command: &str, args: &[String]) -> bool {
    basename_token(command) == "copilot" && args.iter().any(|a| a == "--acp")
}

/// Ports `isQoderAcpCommand`. Not currently re-exported: nothing outside
/// this module needs the predicate by name yet (call sites only check the
/// `"qodercli"` basename directly), kept for parity with acpx and future
/// Qoder-specific quirks.
#[allow(dead_code)]
pub fn is_qoder_acp_command(command: &str, args: &[String]) -> bool {
    basename_token(command) == "qodercli" && args.iter().any(|a| a == "--acp")
}

/// Ports `isCursorAcpCommand`.
pub fn is_cursor_acp_command(command: &str, args: &[String]) -> bool {
    let token = basename_token(command);
    token == "cursor-agent" || (token == "agent" && args.iter().any(|a| a == "acp"))
}

/// Ports `isDevinAcpCommand`.
pub fn is_devin_acp_command(command: &str, args: &[String]) -> bool {
    basename_token(command) == "devin"
        && (args.iter().any(|a| a == "acp")
            || args.iter().any(|a| a == "--acp")
            || args.iter().any(|a| a == "--experimental-acp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_claude_by_basename_or_arg() {
        assert!(is_claude_acp_command("claude-agent-acp", &[]));
        assert!(is_claude_acp_command(
            "npx",
            &["@agentclientprotocol/claude-agent-acp".to_string()]
        ));
        assert!(!is_claude_acp_command("npx", &["other".to_string()]));
    }

    #[test]
    fn detects_cursor_by_basename_or_agent_acp_arg() {
        assert!(is_cursor_acp_command("cursor-agent", &[]));
        assert!(is_cursor_acp_command("agent", &["acp".to_string()]));
        assert!(!is_cursor_acp_command("agent", &[]));
    }

    #[test]
    fn detects_gemini_requires_acp_flag() {
        assert!(is_gemini_acp_command("gemini", &["--acp".to_string()]));
        assert!(!is_gemini_acp_command("gemini", &[]));
    }

    #[test]
    fn detects_copilot_requires_acp_flag() {
        assert!(is_copilot_acp_command("copilot", &["--acp".to_string()]));
        assert!(!is_copilot_acp_command("copilot", &[]));
    }

    #[test]
    fn detects_devin_any_acp_spelling() {
        assert!(is_devin_acp_command("devin", &["acp".to_string()]));
        assert!(is_devin_acp_command(
            "devin",
            &["--experimental-acp".to_string()]
        ));
        assert!(!is_devin_acp_command("devin", &[]));
    }
}
