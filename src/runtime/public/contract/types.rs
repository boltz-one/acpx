//! Plain data types ported from `contract.ts`: prompt/session-mode enums,
//! the `AcpRuntimeHandle`/`*Input` shapes passed into the runtime, and the
//! status/capability/turn-result shapes handed back out. Kept separate from
//! [`super::turn`] (the one type in this family with real behavior) and
//! [`super::registry`]/[`super::options`] (trait-backed, construction-heavy
//! shapes).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::runtime::engine::session_options::SessionAgentOptions;
use crate::runtime::public::events::{
    AcpRuntimeAvailableCommand, AcpRuntimeUsageBreakdown, AcpRuntimeUsageCost,
};

/// Ports `AcpRuntimePromptMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpRuntimePromptMode {
    Prompt,
    Steer,
}

/// Ports `AcpRuntimeSessionMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpRuntimeSessionMode {
    Persistent,
    Oneshot,
}

/// Ports `AcpRuntimeControl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpRuntimeControl {
    SetMode,
    SetConfigOption,
    Status,
}

impl AcpRuntimeControl {
    pub fn as_str(self) -> &'static str {
        match self {
            AcpRuntimeControl::SetMode => "session/set_mode",
            AcpRuntimeControl::SetConfigOption => "session/set_config_option",
            AcpRuntimeControl::Status => "session/status",
        }
    }
}

/// Ports `AcpRuntimeHandle`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpRuntimeHandle {
    pub session_key: String,
    pub backend: String,
    pub runtime_session_name: String,
    pub cwd: Option<String>,
    pub acpx_record_id: Option<String>,
    pub backend_session_id: Option<String>,
    pub agent_session_id: Option<String>,
}

/// Ports `AcpRuntimeEnsureInput`.
pub struct AcpRuntimeEnsureInput {
    pub session_key: String,
    pub agent: String,
    pub mode: AcpRuntimeSessionMode,
    pub resume_session_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub session_options: Option<SessionAgentOptions>,
}

/// Ports `AcpRuntimeTurnAttachment`.
#[derive(Debug, Clone)]
pub struct AcpRuntimeTurnAttachment {
    pub media_type: String,
    pub data: String,
}

/// Ports `AcpRuntimeTurnInput`.
pub struct AcpRuntimeTurnInput {
    pub handle: AcpRuntimeHandle,
    pub text: String,
    pub attachments: Vec<AcpRuntimeTurnAttachment>,
    pub mode: AcpRuntimePromptMode,
    pub request_id: String,
    pub timeout_ms: Option<u64>,
}

/// Ports `AcpRuntimeCapabilities`.
#[derive(Debug, Clone)]
pub struct AcpRuntimeCapabilities {
    pub controls: Vec<AcpRuntimeControl>,
    pub config_option_keys: Option<Vec<String>>,
}

/// Ports `AcpRuntimeSessionModels`.
#[derive(Debug, Clone, Default)]
pub struct AcpRuntimeSessionModels {
    pub current_model_id: Option<String>,
    pub available_model_ids: Vec<String>,
}

/// Ports `AcpRuntimeSessionUsage`.
#[derive(Debug, Clone, Default)]
pub struct AcpRuntimeSessionUsage {
    pub cumulative: Option<AcpRuntimeUsageBreakdown>,
    pub cost: Option<AcpRuntimeUsageCost>,
    pub per_request: HashMap<String, AcpRuntimeUsageBreakdown>,
}

/// Ports `AcpRuntimeStatus`.
#[derive(Debug, Clone, Default)]
pub struct AcpRuntimeStatus {
    pub summary: Option<String>,
    pub acpx_record_id: Option<String>,
    pub backend_session_id: Option<String>,
    pub agent_session_id: Option<String>,
    pub models: Option<AcpRuntimeSessionModels>,
    pub usage: Option<AcpRuntimeSessionUsage>,
    pub available_commands: Option<Vec<AcpRuntimeAvailableCommand>>,
}

/// Ports `AcpRuntimeDoctorReport`.
#[derive(Debug, Clone)]
pub struct AcpRuntimeDoctorReport {
    pub ok: bool,
    pub code: Option<String>,
    pub message: String,
    pub install_command: Option<String>,
    pub details: Vec<String>,
}

/// Ports `AcpRuntimeTurnResultError`.
#[derive(Debug, Clone)]
pub struct AcpRuntimeTurnResultError {
    pub message: String,
    pub code: Option<String>,
    pub detail_code: Option<String>,
    pub retryable: Option<bool>,
}

/// Ports `AcpRuntimeTurnResult`.
#[derive(Debug, Clone)]
pub enum AcpRuntimeTurnResult {
    Completed { stop_reason: Option<String> },
    Cancelled { stop_reason: Option<String> },
    Failed { error: AcpRuntimeTurnResultError },
}
