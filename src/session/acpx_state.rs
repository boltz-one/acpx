//! acpx-equivalent extra session state: desired-vs-current mode/model,
//! config options, and available commands.
//!
//! Ports `SessionAcpxState` and `SessionAvailableCommand` from
//! `others/acpx/src/types.ts`. `config_options` uses the real
//! `agent_client_protocol` schema type directly (as acpx's own type does,
//! importing `SessionConfigOption` from `@agentclientprotocol/sdk`) rather
//! than a locally re-derived shape.

use std::collections::HashMap;

use agent_client_protocol::schema::v1::SessionConfigOption;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAvailableCommand {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub has_input: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelControl {
    ConfigOption,
    LegacySetModel,
}

/// Ports `SessionAcpxState.session_options.system_prompt`'s
/// `string | { append: string }` union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemPromptOption {
    Direct(String),
    Append { append: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionOptions {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub system_prompt: Option<SystemPromptOption>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
}

/// Ports `SessionAcpxState`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionAcpxState {
    #[serde(default)]
    pub reset_on_next_ensure: Option<bool>,
    #[serde(default)]
    pub current_mode_id: Option<String>,
    #[serde(default)]
    pub desired_mode_id: Option<String>,
    #[serde(default)]
    pub desired_config_options: Option<HashMap<String, String>>,
    #[serde(default)]
    pub current_model_id: Option<String>,
    #[serde(default)]
    pub available_models: Option<Vec<String>>,
    #[serde(default)]
    pub model_control: Option<ModelControl>,
    #[serde(default)]
    pub available_commands: Option<Vec<SessionAvailableCommand>>,
    #[serde(default)]
    pub config_options: Option<Vec<SessionConfigOption>>,
    #[serde(default)]
    pub session_options: Option<SessionOptions>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_control_uses_snake_case_wire_values() {
        assert_eq!(
            serde_json::to_string(&ModelControl::ConfigOption).unwrap(),
            "\"config_option\""
        );
        assert_eq!(
            serde_json::to_string(&ModelControl::LegacySetModel).unwrap(),
            "\"legacy_set_model\""
        );
    }

    #[test]
    fn system_prompt_accepts_string_or_append_object() {
        let direct: SystemPromptOption = serde_json::from_value(serde_json::json!("hi")).unwrap();
        assert_eq!(direct, SystemPromptOption::Direct("hi".into()));

        let append: SystemPromptOption =
            serde_json::from_value(serde_json::json!({"append": "more"})).unwrap();
        assert_eq!(
            append,
            SystemPromptOption::Append {
                append: "more".into()
            }
        );
    }

    #[test]
    fn empty_state_round_trips() {
        let state = SessionAcpxState::default();
        let value = serde_json::to_value(&state).unwrap();
        let back: SessionAcpxState = serde_json::from_value(value).unwrap();
        assert_eq!(state, back);
    }
}
