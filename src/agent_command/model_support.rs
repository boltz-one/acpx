//! Ports `others/acpx/src/acp/model-support.ts`'s model-state extraction.
//!
//! acpx parses `configOptions`/legacy `models` metadata out of untyped JSON
//! because it receives raw JSON-RPC payloads. This crate gets typed
//! `SessionConfigOption`s from `agent-client-protocol-schema` directly (see
//! ADR-1), so [`model_state_from_config_options`] matches on the typed enum
//! instead of re-deriving JSON shape checks. Model-request validation
//! (`assertRequestedModelSupported` et al.) lives in `model_request.rs` to
//! keep both files under the line-count convention.

use agent_client_protocol::schema::v1::{
    SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single selectable model. Ports `SessionModelState["availableModels"][number]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableModel {
    pub model_id: String,
    pub name: String,
}

/// Ports `SessionModelState`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionModelState {
    pub config_id: Option<String>,
    pub current_model_id: String,
    pub available_models: Vec<AvailableModel>,
}

fn flatten_select_options(options: &SessionConfigSelectOptions) -> Vec<AvailableModel> {
    match options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .map(|o| AvailableModel {
                model_id: o.value.0.to_string(),
                name: o.name.clone(),
            })
            .collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|g| {
                g.options.iter().map(|o| AvailableModel {
                    model_id: o.value.0.to_string(),
                    name: o.name.clone(),
                })
            })
            .collect(),
        // `SessionConfigSelectOptions` is `#[non_exhaustive]`: a future ACP
        // schema revision may add option shapes this crate doesn't know
        // about yet. Treat unknown shapes as "no models advertised" rather
        // than failing to compile against a schema point release.
        _ => Vec::new(),
    }
}

fn is_model_select_option(option: &SessionConfigOption) -> bool {
    matches!(option.category, Some(SessionConfigOptionCategory::Model))
        || option.id.0.as_ref() == "model"
}

/// Ports `modelStateFromConfigOptions`.
pub fn model_state_from_config_options(
    config_options: &[SessionConfigOption],
) -> Option<SessionModelState> {
    config_options.iter().find_map(|option| {
        if !is_model_select_option(option) {
            return None;
        }
        let SessionConfigKind::Select(select) = &option.kind else {
            return None;
        };
        Some(SessionModelState {
            config_id: Some(option.id.0.to_string()),
            current_model_id: select.current_value.0.to_string(),
            available_models: flatten_select_options(&select.options),
        })
    })
}

/// Ports `modelStateFromLegacyResponse`: some pre-`configOptions` adapters
/// (older Claude Code ACP builds) attach a `_meta.models` object instead.
/// `meta` is the response's raw `_meta` JSON, since legacy metadata has no
/// typed schema representation.
pub fn model_state_from_legacy_response(meta: Option<&Value>) -> Option<SessionModelState> {
    let models = meta?.get("models")?;
    let current_model_id = models.get("currentModelId")?.as_str()?.to_string();
    let available_models = models
        .get("availableModels")?
        .as_array()?
        .iter()
        .filter_map(|entry| {
            Some(AvailableModel {
                model_id: entry.get("modelId")?.as_str()?.to_string(),
                name: entry.get("name")?.as_str()?.to_string(),
            })
        })
        .collect();
    Some(SessionModelState {
        config_id: None,
        current_model_id,
        available_models,
    })
}

/// Ports `modelStateFromSessionResponse`.
pub fn model_state_from_session_response(
    config_options: &[SessionConfigOption],
    legacy_meta: Option<&Value>,
) -> Option<SessionModelState> {
    model_state_from_config_options(config_options)
        .or_else(|| model_state_from_legacy_response(legacy_meta))
}

/// Ports `formatAvailableModelIds`.
pub fn format_available_model_ids(models: Option<&SessionModelState>) -> String {
    let ids: Vec<&str> = models
        .map(|m| {
            m.available_models
                .iter()
                .map(|model| model.model_id.trim())
                .filter(|id| !id.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        "none advertised".to_string()
    } else {
        ids.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        SessionConfigId, SessionConfigSelect, SessionConfigSelectOption,
    };

    pub(crate) fn model_option(current: &'static str) -> SessionConfigOption {
        SessionConfigOption::new(
            SessionConfigId::new("model"),
            "Model",
            SessionConfigKind::Select(SessionConfigSelect::new(
                current,
                vec![
                    SessionConfigSelectOption::new("default-model", "default-model"),
                    SessionConfigSelectOption::new("fast-model", "fast-model"),
                ],
            )),
        )
    }

    #[test]
    fn extracts_model_state_from_config_options() {
        let options = vec![model_option("default-model")];
        let state = model_state_from_config_options(&options).unwrap();
        assert_eq!(state.current_model_id, "default-model");
        assert_eq!(state.available_models.len(), 2);
    }

    #[test]
    fn legacy_response_metadata_is_parsed() {
        let meta = serde_json::json!({
            "models": {
                "currentModelId": "default-model",
                "availableModels": [{"modelId": "default-model", "name": "Default"}]
            }
        });
        let state = model_state_from_legacy_response(Some(&meta)).unwrap();
        assert_eq!(state.available_models[0].model_id, "default-model");
    }
}
