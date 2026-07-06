//! Session conversation model: message history, truncation limits, and the
//! functions that append to/trim it.
//!
//! Ports `others/acpx/src/session/conversation-model.ts` (941 lines, the
//! largest single source file in phase-05's scope), split across
//! submodules per the phase-05 Architecture doc (further split beyond the
//! doc's suggested layout to stay under this crate's per-file line
//! convention):
//! - [`limits`] — the `MAX_RUNTIME_*` constants.
//! - [`message`] / [`conversation`] — the `SessionMessage` family and
//!   `SessionConversation` type definitions (acpx defines these in
//!   `types.ts`; grouped here since conversation-model.ts is their only
//!   consumer).
//! - [`trim`] — `trimConversationForRuntime` and friends.
//! - [`agent_content`] — `ensureAgentMessage`/`appendAgentText`/
//!   `appendAgentThinking`/`contentToUserContent`.
//! - [`record`] — `recordPromptSubmission`/`recordPromptResponseUsage`/
//!   `recordClientOperation`/`hasAgentReplyAfterPrompt`.
//! - [`tool_use`] — `ensureToolUseContent`/`upsertToolResult` helpers.
//! - [`tool_call`] — `applyToolCallUpdate`.
//! - [`session_update`] — the `applySessionUpdate` dispatch table
//!   (`recordSessionUpdate`).
//!
//! `appendLegacyHistory`/`LegacyHistoryEntry` (acpx's pre-acpx-session-format
//! migration helper) is intentionally not ported: this crate has no
//! predecessor on-disk format to migrate from, so it would be dead code
//! (YAGNI) — see the phase-05 implementation report for this deviation.

pub mod agent_content;
pub mod conversation;
pub mod limits;
pub mod message;
pub mod record;
pub mod session_update;
pub mod tool_call;
pub mod tool_use;
pub mod trim;

pub use agent_content::{
    InboundContent, append_agent_text, append_agent_thinking, ensure_agent_message,
};
pub use conversation::{
    SessionConversation, SessionTokenUsage, SessionUsageCost, clone_session_conversation,
    create_session_conversation, iso_now,
};
pub use message::{
    SessionAgentContent, SessionAgentMessage, SessionMessage, SessionMessageAudio,
    SessionMessageImage, SessionMessageImageSize, SessionToolResult, SessionToolResultContent,
    SessionToolUse, SessionUserContent, SessionUserMessage,
};
pub use record::{
    has_agent_reply_after_prompt, record_client_operation, record_prompt_response_usage,
    record_prompt_submission,
};
pub use session_update::{SessionUpdateInput, record_session_update};
pub use tool_call::{ToolCallUpdateInput, apply_tool_call_update};
pub use trim::{trim_conversation_for_runtime, trim_runtime_text};
