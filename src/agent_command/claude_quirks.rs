//! Claude Code ACP runtime quirks ported from
//! `others/acpx/src/acp/agent-command.ts`: the `session/new` creation-timeout
//! duration override and the diagnostic message attached to
//! [`crate::error::AcpError::ClaudeAcpSessionCreateTimeout`].
//!
//! Claude Code's ACP adapter can hang indefinitely on `session/new` (a real
//! upstream behavior acpx works around) — usually when it is waiting on
//! interactive permission approval or auth that never arrives in a
//! non-interactive run. This bounds it at 60s by default, matching acpx's
//! `resolveClaudeAcpSessionCreateTimeoutMs`. The env var keeps acpx's
//! `ACPX_`-prefixed name for consistency with [`super::gemini_quirks`]'s
//! analogous `ACPX_GEMINI_ACP_STARTUP_TIMEOUT_MS`.

use std::time::Duration;

const CLAUDE_ACP_SESSION_CREATE_TIMEOUT_MS: u64 = 60_000;

/// Ports `resolveClaudeAcpSessionCreateTimeoutMs`: env override
/// `ACPX_CLAUDE_ACP_SESSION_CREATE_TIMEOUT_MS`, default 60s. Mirrors
/// [`super::gemini_quirks::resolve_gemini_acp_startup_timeout_ms`]'s
/// parse-and-validate shape exactly.
pub fn resolve_claude_acp_session_create_timeout_ms() -> Duration {
    resolve_claude_acp_session_create_timeout_ms_from(
        std::env::var("ACPX_CLAUDE_ACP_SESSION_CREATE_TIMEOUT_MS")
            .ok()
            .as_deref(),
    )
}

fn resolve_claude_acp_session_create_timeout_ms_from(raw: Option<&str>) -> Duration {
    let parsed = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0);
    match parsed {
        Some(ms) => Duration::from_millis(ms.round() as u64),
        None => Duration::from_millis(CLAUDE_ACP_SESSION_CREATE_TIMEOUT_MS),
    }
}

/// Ports `buildClaudeAcpSessionCreateTimeoutMessage`: the diagnostic carried
/// by [`crate::error::AcpError::ClaudeAcpSessionCreateTimeout`]. A static
/// string (no subprocess probe, unlike Gemini's version-aware message).
pub fn build_claude_acp_session_create_timeout_message() -> String {
    [
        "Claude Code ACP session creation timed out before session/new completed.",
        "This usually means the Claude Code ACP adapter is blocked waiting on interactive permission approval or auth.",
        "Try an approve-all permission mode (or a non-interactive permission policy that does not deny), and ensure Claude Code auth is configured for non-interactive use; the runtime will otherwise fall back to creating a fresh session.",
    ]
    .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_defaults_when_env_unset_or_invalid() {
        assert_eq!(
            resolve_claude_acp_session_create_timeout_ms_from(None),
            Duration::from_millis(CLAUDE_ACP_SESSION_CREATE_TIMEOUT_MS)
        );
        assert_eq!(
            resolve_claude_acp_session_create_timeout_ms_from(Some("not-a-number")),
            Duration::from_millis(CLAUDE_ACP_SESSION_CREATE_TIMEOUT_MS)
        );
        assert_eq!(
            resolve_claude_acp_session_create_timeout_ms_from(Some("0")),
            Duration::from_millis(CLAUDE_ACP_SESSION_CREATE_TIMEOUT_MS)
        );
    }

    #[test]
    fn timeout_honors_valid_env_override() {
        assert_eq!(
            resolve_claude_acp_session_create_timeout_ms_from(Some("1500")),
            Duration::from_millis(1500)
        );
    }

    #[test]
    fn message_mentions_timeout_and_guidance() {
        let message = build_claude_acp_session_create_timeout_message();
        assert!(message.contains("timed out"));
        assert!(message.contains("approve-all"));
    }
}
