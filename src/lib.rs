//! Agent Client Protocol (ACP) client + session runtime, in Rust.
//!
//! A `smol`-based, embeddable library for driving ACP coding agents
//! (`claude-agent-acp`, `codex-acp`, Gemini, Copilot, …): the `initialize`
//! handshake, session create/resume/load with reconnect, per-session prompt
//! queueing, permission policy + escalation, filesystem/terminal client
//! tools, and JSON session persistence. It ports the "core client + session
//! persistence + in-process queueing" surface of the `acpx` TypeScript CLI;
//! the CLI/commander layer, the cross-process IPC queue daemon, and the
//! flows DSL are intentionally out of scope.
//!
//! Architecture decisions are recorded in `docs/decisions/`.

/// Vendored subprocess spawn/kill primitives (see the module docs). Kept
/// internal — not part of the public API.
mod util;

pub mod agent_command;
pub mod agent_session_id;
pub mod auth_env;
pub mod client;
pub mod control;
pub mod error;
pub mod error_normalization;
pub mod error_shapes;
pub mod filesystem;
pub mod jsonrpc_gap;
pub mod mcp_servers;
#[cfg(feature = "perf-metrics")]
pub mod perf_metrics;
pub mod permissions;
mod platform;
pub mod queue;
pub mod runtime;
pub mod session;
pub mod session_control_errors;
pub mod terminal;
pub mod types;
pub mod version;

pub use error::{AcpError, Result};
