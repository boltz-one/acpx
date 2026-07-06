//! Conversation message/content types.
//!
//! Ports `SessionMessage`, `SessionUserContent`, `SessionAgentContent`,
//! `SessionToolUse`, `SessionToolResult`, `SessionToolResultContent`,
//! `SessionUserMessage`, `SessionAgentMessage`, `SessionMessageImage`,
//! `SessionMessageAudio` from `others/acpx/src/types.ts`.
//!
//! acpx represents these as single-key-object (or bare-string) unions, e.g.
//! `{ Text: string } | { Mention: {...} } | ...` and `"Resume"` as a literal
//! variant of `SessionMessage`. Rust's *default* externally-tagged enum
//! representation produces exactly this shape for a newtype/struct variant
//! (`{"Text": "..."}`) and a unit variant (`"Resume"`), so no `#[serde(tag =
//! ...)]` is needed here — the derive's default output already matches
//! acpx's PascalCase tag keys, which is exactly what
//! `others/acpx/src/persisted-key-policy.ts`'s `ZED_TAG_KEYS` allowlist
//! (`User`, `Agent`, `Resume`, `Text`, `Mention`, ...) exists to permit
//! amid an otherwise-snake_case document (see ADR-5).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMessageImage {
    pub source: String,
    #[serde(default)]
    pub size: Option<SessionMessageImageSize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessageImageSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessageAudio {
    pub source: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionUserContent {
    Text(String),
    Mention { uri: String, content: String },
    Image(SessionMessageImage),
    Audio(SessionMessageAudio),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionToolUse {
    pub id: String,
    pub name: String,
    pub raw_input: String,
    #[serde(default)]
    pub input: Value,
    pub is_input_complete: bool,
    #[serde(default)]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionToolResultContent {
    Text(String),
    Image(SessionMessageImage),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionToolResult {
    pub tool_use_id: String,
    pub tool_name: String,
    pub is_error: bool,
    pub content: SessionToolResultContent,
    #[serde(default)]
    pub output: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionAgentContent {
    Text(String),
    Thinking {
        text: String,
        #[serde(default)]
        signature: Option<String>,
    },
    RedactedThinking(String),
    ToolUse(SessionToolUse),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionUserMessage {
    pub id: String,
    #[serde(default)]
    pub content: Vec<SessionUserContent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionAgentMessage {
    #[serde(default)]
    pub content: Vec<SessionAgentContent>,
    #[serde(default)]
    pub tool_results: HashMap<String, SessionToolResult>,
    #[serde(default)]
    pub reasoning_details: Option<Value>,
}

/// Ports `SessionMessage`. See module docs for why no `#[serde(tag = ...)]`
/// is needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionMessage {
    User(SessionUserMessage),
    Agent(SessionAgentMessage),
    Resume,
}

impl SessionMessage {
    pub fn as_user(&self) -> Option<&SessionUserMessage> {
        match self {
            SessionMessage::User(message) => Some(message),
            _ => None,
        }
    }

    pub fn as_agent(&self) -> Option<&SessionAgentMessage> {
        match self {
            SessionMessage::Agent(message) => Some(message),
            _ => None,
        }
    }

    pub fn as_agent_mut(&mut self) -> Option<&mut SessionAgentMessage> {
        match self {
            SessionMessage::Agent(message) => Some(message),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_serializes_as_bare_string() {
        assert_eq!(
            serde_json::to_string(&SessionMessage::Resume).unwrap(),
            "\"Resume\""
        );
    }

    #[test]
    fn user_message_serializes_under_pascal_case_tag() {
        let message = SessionMessage::User(SessionUserMessage {
            id: "m1".into(),
            content: vec![SessionUserContent::Text("hi".into())],
        });
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["User"]["id"], "m1");
        assert_eq!(value["User"]["content"][0]["Text"], "hi");
    }

    #[test]
    fn tool_use_round_trips() {
        let content = SessionAgentContent::ToolUse(SessionToolUse {
            id: "t1".into(),
            name: "tool_call".into(),
            raw_input: "{}".into(),
            input: Value::Object(Default::default()),
            is_input_complete: true,
            thought_signature: None,
        });
        let value = serde_json::to_value(&content).unwrap();
        let back: SessionAgentContent = serde_json::from_value(value).unwrap();
        assert_eq!(content, back);
    }
}
