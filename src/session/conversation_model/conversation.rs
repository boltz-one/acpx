//! `SessionConversation` + token-usage/cost types.
//!
//! Ports `SessionConversation`, `SessionTokenUsage`, `SessionUsageCost` from
//! `others/acpx/src/types.ts`.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionTokenUsage {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    pub thought_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
}

impl SessionTokenUsage {
    pub fn has_value(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cache_creation_input_tokens.is_some()
            || self.cache_read_input_tokens.is_some()
            || self.thought_tokens.is_some()
            || self.total_tokens.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionUsageCost {
    #[serde(default)]
    pub amount: Option<f64>,
    #[serde(default)]
    pub currency: Option<String>,
}

/// Ports `SessionConversation`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionConversation {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub messages: Vec<super::message::SessionMessage>,
    pub updated_at: String,
    #[serde(default)]
    pub cumulative_token_usage: SessionTokenUsage,
    #[serde(default)]
    pub cumulative_cost: Option<SessionUsageCost>,
    #[serde(default)]
    pub request_token_usage: IndexMap<String, SessionTokenUsage>,
}

/// Current wall-clock time as an ISO-8601 string. Ports the several local
/// `isoNow()` helpers scattered across acpx's session files.
pub fn iso_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Ports `createSessionConversation`.
pub fn create_session_conversation(timestamp: impl Into<Option<String>>) -> SessionConversation {
    SessionConversation {
        title: None,
        messages: Vec::new(),
        updated_at: timestamp.into().unwrap_or_else(iso_now),
        cumulative_token_usage: SessionTokenUsage::default(),
        cumulative_cost: None,
        request_token_usage: IndexMap::new(),
    }
}

/// Ports `cloneSessionConversation`. A plain `Clone::clone` already does the
/// deep copy `structuredClone` performed in TS (every field here is owned,
/// no shared references), so this exists only for call-site parity with
/// acpx and to handle the "no conversation yet" case the same way.
pub fn clone_session_conversation(
    conversation: Option<&SessionConversation>,
) -> SessionConversation {
    conversation
        .cloned()
        .unwrap_or_else(|| create_session_conversation(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_uses_provided_timestamp() {
        let conversation = create_session_conversation(Some("2026-01-01T00:00:00Z".to_string()));
        assert_eq!(conversation.updated_at, "2026-01-01T00:00:00Z");
        assert!(conversation.messages.is_empty());
    }

    #[test]
    fn clone_of_none_creates_fresh_conversation() {
        let conversation = clone_session_conversation(None);
        assert!(conversation.messages.is_empty());
        assert!(conversation.cumulative_cost.is_none());
    }

    #[test]
    fn missing_token_usage_fields_default_to_none() {
        let usage: SessionTokenUsage = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!usage.has_value());
    }
}
