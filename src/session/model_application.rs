//! Ports the pure (no live-client) half of
//! `others/acpx/src/session/model-application.ts`:
//! `currentModelIdFromSetModelResponse`.
//!
//! `applyRequestedModelIfAdvertised` is **not** ported here: it takes a live
//! `AcpClient` and calls `client.setSessionModel(...)`, which is runtime
//! orchestration that belongs to Phase 4 (the runtime engine, which owns
//! the live `agent-client-protocol` connection this phase does not depend
//! on — see plan.md's phase table). Phase 4 is expected to call
//! [`current_model_id_from_set_model_response`] after issuing its own
//! `session/set_config_option` request, mirroring acpx's split between
//! this file's pure helper and its live-client half.

use agent_client_protocol::schema::v1::SessionConfigOption;

use crate::agent_command::model_support::model_state_from_config_options;

/// Ports `currentModelIdFromSetModelResponse`.
pub fn current_model_id_from_set_model_response(
    response_config_options: Option<&[SessionConfigOption]>,
    fallback_model_id: Option<&str>,
) -> Option<String> {
    response_config_options
        .and_then(model_state_from_config_options)
        .map(|models| models.current_model_id)
        .or_else(|| fallback_model_id.map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        SessionConfigId, SessionConfigKind, SessionConfigSelect, SessionConfigSelectOption,
    };

    #[test]
    fn falls_back_when_response_has_no_model_option() {
        assert_eq!(
            current_model_id_from_set_model_response(None, Some("fallback")),
            Some("fallback".to_string())
        );
    }

    #[test]
    fn prefers_response_advertised_model() {
        let option = SessionConfigOption::new(
            SessionConfigId::new("model"),
            "Model",
            SessionConfigKind::Select(SessionConfigSelect::new(
                "gpt-5",
                vec![SessionConfigSelectOption::new("gpt-5", "gpt-5")],
            )),
        );
        assert_eq!(
            current_model_id_from_set_model_response(Some(&[option]), Some("fallback")),
            Some("gpt-5".to_string())
        );
    }
}
