//! Truncation limits applied to persisted conversation state.
//!
//! Ports the constant block at the top of
//! `others/acpx/src/session/conversation-model.ts` verbatim — these numbers
//! are load-bearing (they bound on-disk session file growth) and must not
//! be "improved" without re-checking acpx's behavior.

/// Maximum number of messages kept in `SessionConversation.messages`. Older
/// messages are dropped from the front once this is exceeded.
pub const MAX_RUNTIME_MESSAGES: usize = 200;

/// Maximum characters kept in an agent `Text` content block (and in a user
/// `Text` content block on prompt submission).
pub const MAX_RUNTIME_AGENT_TEXT_CHARS: usize = 8_000;

/// Maximum characters kept in an agent `Thinking` content block's text.
pub const MAX_RUNTIME_THINKING_CHARS: usize = 4_000;

/// Maximum characters kept in a tool call's `raw_input` and in a tool
/// result's text/string output.
pub const MAX_RUNTIME_TOOL_IO_CHARS: usize = 4_000;

/// Maximum number of entries kept in `SessionConversation.request_token_usage`.
pub const MAX_RUNTIME_REQUEST_TOKEN_USAGE: usize = 100;
