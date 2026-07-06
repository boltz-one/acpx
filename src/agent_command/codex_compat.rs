//! Ports `others/acpx/src/acp/codex-compat.ts` (20 lines in the TS source).

use super::command_args::basename_token;
use std::sync::OnceLock;

/// Ports `isCodexAcpCommand`.
pub fn is_codex_acp_command(command: &str, args: &[String]) -> bool {
    basename_token(command) == "codex-acp" || args.iter().any(|arg| arg.contains("codex-acp"))
}

fn legacy_zed_codex_acp_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| regex::Regex::new(r"@zed-industries/codex-acp\b").unwrap())
}

/// Ports `isLegacyZedCodexAcpInvocation`.
pub fn is_legacy_zed_codex_acp_invocation(agent_command: &str) -> bool {
    legacy_zed_codex_acp_pattern().is_match(agent_command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_codex_acp_by_basename() {
        assert!(is_codex_acp_command("codex-acp", &[]));
    }

    #[test]
    fn detects_codex_acp_in_args() {
        assert!(is_codex_acp_command(
            "npx",
            &[
                "-y".to_string(),
                "@agentclientprotocol/codex-acp".to_string()
            ]
        ));
    }

    #[test]
    fn detects_legacy_zed_invocation() {
        assert!(is_legacy_zed_codex_acp_invocation(
            "npx -y @zed-industries/codex-acp@0.1.0"
        ));
        assert!(!is_legacy_zed_codex_acp_invocation("npx -y codex-acp"));
    }
}
