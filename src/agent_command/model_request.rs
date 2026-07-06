//! Model-request validation ported from the second half of
//! `others/acpx/src/acp/model-support.ts` (`assertRequestedModelSupported`
//! and friends). Split from `model_support.rs` to stay under 200 lines.

use super::agent_detect::{is_claude_acp_command, is_cursor_acp_command};
use super::command_args::split_command_line;
use super::model_support::{SessionModelState, format_available_model_ids};

/// Ports `RequestedModelUnsupportedReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestedModelUnsupportedReason {
    MissingCapability,
    UnadvertisedModel,
}

/// Ports `RequestedModelUnsupportedError`. A dedicated error type (rather
/// than a new [`crate::AcpError`] variant) since this module owns model
/// support but not `error.rs`; callers wrap it as `AcpError::Other(err.into())`.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct RequestedModelUnsupportedError {
    pub message: String,
    pub reason: RequestedModelUnsupportedReason,
}

/// Ports `supportsLegacyClaudeCodeModelMetadata`.
pub fn supports_legacy_claude_code_model_metadata(agent_command: Option<&str>) -> bool {
    let Some(agent_command) = agent_command else {
        return false;
    };
    let Ok(parts) = split_command_line(agent_command) else {
        return false;
    };
    is_claude_acp_command(&parts.command, &parts.args)
}

fn is_cursor_acp_command_for_model_alias(agent_command: Option<&str>) -> bool {
    let Some(agent_command) = agent_command else {
        return false;
    };
    let Ok(parts) = split_command_line(agent_command) else {
        return false;
    };
    is_cursor_acp_command(&parts.command, &parts.args)
}

/// Ports `resolveRequestedModelId`: Cursor ACP advertises model ids with a
/// bracketed suffix (e.g. `gpt-5[high]`); if the caller requested the bare
/// alias and exactly one advertised id starts with `alias[`, use that.
pub fn resolve_requested_model_id(
    requested_model: &str,
    models: Option<&SessionModelState>,
    agent_command: Option<&str>,
) -> String {
    let Some(models) = models else {
        return requested_model.to_string();
    };
    if !is_cursor_acp_command_for_model_alias(agent_command) {
        return requested_model.to_string();
    }
    if models
        .available_models
        .iter()
        .any(|m| m.model_id == requested_model)
    {
        return requested_model.to_string();
    }
    let prefix = format!("{requested_model}[");
    let candidates: Vec<&str> = models
        .available_models
        .iter()
        .map(|m| m.model_id.as_str())
        .filter(|id| id.starts_with(&prefix))
        .collect();
    match candidates.as_slice() {
        [only] => (*only).to_string(),
        _ => requested_model.to_string(),
    }
}

/// Ports `assertRequestedModelSupported`. Returns `Ok(Some(warning))` when
/// the model is accepted but worth surfacing a note about (alias resolution,
/// forwarding to a legacy adapter), `Ok(None)` when unremarkable, and `Err`
/// when the agent definitely can't honor the request.
pub fn assert_requested_model_supported(
    requested_model: &str,
    models: Option<&SessionModelState>,
    agent_command: Option<&str>,
    is_replay: bool,
) -> Result<Option<String>, RequestedModelUnsupportedError> {
    let action = if is_replay {
        "replay saved model"
    } else {
        "apply --model"
    };
    let Some(models) = models else {
        if supports_legacy_claude_code_model_metadata(agent_command) {
            return Ok(None);
        }
        return Err(RequestedModelUnsupportedError {
            message: format!(
                "Cannot {action} \"{requested_model}\": the ACP agent did not advertise model support through a session config option or legacy models metadata, and the adapter does not support a startup model flag."
            ),
            reason: RequestedModelUnsupportedReason::MissingCapability,
        });
    };

    if models
        .available_models
        .iter()
        .any(|m| m.model_id == requested_model)
    {
        return Ok(None);
    }

    let resolved = resolve_requested_model_id(requested_model, Some(models), agent_command);
    if resolved != requested_model {
        return Ok(Some(format!(
            "Cursor ACP advertised \"{resolved}\" for requested model \"{requested_model}\"; using the advertised id."
        )));
    }
    if supports_legacy_claude_code_model_metadata(agent_command) {
        return Ok(Some(format!(
            "requested model \"{requested_model}\" was not in the Claude ACP advertised model list ({}); forwarding it to Claude Code so the adapter can accept or reject it.",
            format_available_model_ids(Some(models))
        )));
    }
    Err(RequestedModelUnsupportedError {
        message: format!(
            "Cannot {action} \"{requested_model}\": the ACP agent did not advertise that model. Available models: {}.",
            format_available_model_ids(Some(models))
        ),
        reason: RequestedModelUnsupportedReason::UnadvertisedModel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_command::model_support::model_state_from_config_options;
    use agent_client_protocol::schema::v1::{
        SessionConfigId, SessionConfigKind, SessionConfigOption, SessionConfigSelect,
        SessionConfigSelectOption,
    };

    fn model_option(current: &'static str) -> SessionConfigOption {
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
    fn supported_model_passes_without_warning() {
        let options = vec![model_option("default-model")];
        let state = model_state_from_config_options(&options);
        let result = assert_requested_model_supported("fast-model", state.as_ref(), None, false);
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn unadvertised_model_without_legacy_support_errors() {
        let options = vec![model_option("default-model")];
        let state = model_state_from_config_options(&options);
        let result = assert_requested_model_supported("nope", state.as_ref(), None, false);
        assert!(result.is_err());
    }

    #[test]
    fn missing_capability_without_legacy_support_errors() {
        let result = assert_requested_model_supported("gpt-5", None, None, false);
        assert_eq!(
            result.unwrap_err().reason,
            RequestedModelUnsupportedReason::MissingCapability
        );
    }
}
