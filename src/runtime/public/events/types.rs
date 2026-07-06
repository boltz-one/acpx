//! `AcpRuntimeEvent` and its constituent shapes. Ports `contract.ts`'s
//! `AcpRuntimeEvent` shape; parsing a raw `session/update` into one of these
//! lives in [`super::parse`].

use agent_client_protocol::schema::v1::{ToolCallContent, ToolCallLocation, ToolKind};

/// Ports `AcpSessionUpdateTag`. acpx's version is an open string union
/// (`| (string & {})`); a plain `String` is the direct Rust analog since
/// this crate has no need to exhaustively match on it (tags are attached to
/// events purely for the UI's benefit).
pub type AcpSessionUpdateTag = String;

/// Which stream a `text_delta` event belongs to. Ports the `"output" |
/// "thought"` union on `AcpRuntimeEvent`'s `text_delta` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpRuntimeTextStream {
    Output,
    Thought,
}

/// Ports `AcpRuntimeUsageCost`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AcpRuntimeUsageCost {
    pub amount: Option<f64>,
    pub currency: Option<String>,
}

/// Ports `AcpRuntimeUsageBreakdown`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcpRuntimeUsageBreakdown {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cached_write_tokens: Option<u64>,
    pub thought_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

impl AcpRuntimeUsageBreakdown {
    pub(crate) fn is_empty(&self) -> bool {
        self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.cached_read_tokens.is_none()
            && self.cached_write_tokens.is_none()
            && self.thought_tokens.is_none()
            && self.total_tokens.is_none()
    }
}

/// Ports `AcpRuntimeAvailableCommand`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpRuntimeAvailableCommand {
    pub name: String,
    pub description: Option<String>,
    pub has_input: Option<bool>,
}

/// Ports `AcpRuntimeEvent`. The `done`/`error` compatibility-terminal
/// variants exist only for [`crate::runtime::engine::manager::AcpRuntime::run_turn`]'s
/// legacy shim (see that method's docs) — [`super::parse::parse_session_update`]
/// never produces them; they're built directly from an
/// [`crate::runtime::public::contract::AcpRuntimeTurnResult`] instead.
#[derive(Debug, Clone, PartialEq)]
pub enum AcpRuntimeEvent {
    TextDelta {
        text: String,
        stream: AcpRuntimeTextStream,
        tag: Option<AcpSessionUpdateTag>,
    },
    Status {
        text: String,
        tag: Option<AcpSessionUpdateTag>,
        used: Option<u64>,
        size: Option<u64>,
        cost: Option<AcpRuntimeUsageCost>,
        breakdown: Option<AcpRuntimeUsageBreakdown>,
        available_commands: Option<Vec<AcpRuntimeAvailableCommand>>,
    },
    ToolCall {
        text: String,
        tag: Option<AcpSessionUpdateTag>,
        tool_call_id: Option<String>,
        status: Option<String>,
        title: Option<String>,
        kind: Option<ToolKind>,
        locations: Vec<ToolCallLocation>,
        raw_input: Option<serde_json::Value>,
        raw_output: Option<serde_json::Value>,
        content: Vec<ToolCallContent>,
    },
    /// Compatibility terminal event; see the enum's module docs.
    Done { stop_reason: Option<String> },
    /// Compatibility failure event; see the enum's module docs.
    Error {
        message: String,
        code: Option<String>,
        detail_code: Option<String>,
        retryable: Option<bool>,
    },
    /// Ports acpx's `ClientOperation` type (`types.ts` L130-147), surfaced
    /// at the same tier as a `session/update`-derived event (see
    /// `manager.ts:1101-1109`'s dual `onClientOperation` handling: persisted
    /// via `record_client_operation` AND streamed as this variant). `method`
    /// mirrors acpx's closed `ClientOperationMethod` union (e.g.
    /// `"fs/read_text_file"`, `"terminal/create"`) and `status` its
    /// `ClientOperationStatus` union (`"running"`/`"completed"`/`"failed"`)
    /// as plain strings, matching this file's existing open-tag convention
    /// ([`AcpSessionUpdateTag`]) rather than adding narrow enums no other
    /// variant here uses.
    ///
    /// `TODO(gap-20-wiring)`: [`crate::filesystem::FilesystemHandlers`]'s and
    /// [`crate::terminal::TerminalManager`]'s `on_operation` callbacks are
    /// not yet wired to construct/emit this variant — that's the runtime
    /// engine's (`manager_spawn.rs`/`prompt_turn`) responsibility, out of
    /// this phase's file-ownership scope.
    ClientOperation {
        method: String,
        status: String,
        summary: String,
        details: Option<String>,
        timestamp: String,
    },
}
