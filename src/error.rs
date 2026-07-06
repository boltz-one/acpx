//! Error types for the `acp` crate.
//!
//! Ported from `others/acpx/src/errors.ts` (error classes) and
//! `others/acpx/src/acp/jsonrpc-error.ts` (JSON-RPC error code table). The
//! acpx CLI-only queue/IPC error classes (`QueueConnectionError`,
//! `QueueProtocolError`) are intentionally not ported — they belong to the
//! cross-process IPC daemon this crate does not implement (see plan.md scope).

use std::time::Duration;

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, AcpError>;

#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    #[error("session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("session resolution failed: {0}")]
    SessionResolution(String),

    #[error("failed to spawn agent command: {command}")]
    AgentSpawn {
        command: String,
        #[source]
        source: std::io::Error,
    },

    /// Ports the spawn-failure half of `terminal-manager.ts`'s
    /// `terminal/create` (Phase 3) — kept distinct from [`AcpError::AgentSpawn`]
    /// since a terminal command is spawned by (and attributed to) the
    /// session, not this crate's own agent subprocess.
    #[error("failed to spawn terminal command: {command}")]
    TerminalSpawn {
        command: String,
        #[source]
        source: std::io::Error,
    },

    #[error("ACP agent exited before initialize completed (exit={exit_code:?}, signal={signal:?})")]
    AgentStartup {
        command: String,
        exit_code: Option<i32>,
        signal: Option<String>,
        stderr_summary: Option<String>,
    },

    #[error(
        "ACP agent disconnected during request ({reason}, exit={exit_code:?}, signal={signal:?})"
    )]
    AgentDisconnected {
        reason: String,
        exit_code: Option<i32>,
        signal: Option<String>,
    },

    #[error("unsupported prompt content: {0}")]
    UnsupportedPromptContent(String),

    #[error("session resume required: {0}")]
    SessionResumeRequired(String),

    #[error("gemini ACP startup timed out: {0}")]
    GeminiAcpStartupTimeout(String),

    #[error("session mode replay failed: {0}")]
    SessionModeReplay(String),

    #[error("session model replay failed: {0}")]
    SessionModelReplay(String),

    #[error("session config option replay failed: {0}")]
    SessionConfigOptionReplay(String),

    #[error("claude ACP session create timed out: {0}")]
    ClaudeAcpSessionCreateTimeout(String),

    #[error("copilot ACP unsupported: {0}")]
    CopilotAcpUnsupported(String),

    #[error("auth policy error: {0}")]
    AuthPolicy(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("permission prompt unavailable in non-interactive mode")]
    PermissionPromptUnavailable,

    /// Raised by [`crate::control::with_timeout`] on expiry.
    #[error("timed out after {0:?}")]
    Timeout(Duration),

    /// Reserved for cooperative cancellation (Phase 6 queueing); not yet
    /// raised anywhere in this crate.
    #[error("interrupted")]
    Interrupted,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl AcpError {
    /// The JSON-RPC 2.0 error code acpx would report for this error, per
    /// `OUTPUT_ERROR_JSONRPC_CODES` in `jsonrpc-error.ts`. Errors with no
    /// direct table entry fall back to `RUNTIME` (-32603), matching acpx's
    /// own fallback in `buildErrorObject`.
    pub fn json_rpc_code(&self) -> i64 {
        match self {
            AcpError::SessionNotFound { .. } => -32002,
            AcpError::Timeout(_)
            | AcpError::GeminiAcpStartupTimeout(_)
            | AcpError::ClaudeAcpSessionCreateTimeout(_) => -32070,
            AcpError::PermissionDenied(_) => -32071,
            AcpError::PermissionPromptUnavailable => -32072,
            AcpError::UnsupportedPromptContent(_) => -32602,
            _ => -32603,
        }
    }
}
