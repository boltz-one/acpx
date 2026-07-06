//! Folding a live `session/new`/`session/load` config-options response into
//! a persisted [`SessionRecord`]/[`SessionAcpxState`].
//!
//! Ports `others/acpx/src/session/config-options.ts`. acpx's
//! `applyConfigOptionsToRecord` takes a `Pick<SessionCreateResult |
//! SessionLoadResult, "configOptions">` (Phase 4's runtime-engine result
//! types); this port takes the config options slice directly instead, since
//! Phase 5 doesn't define those result types.

use agent_client_protocol::schema::v1::SessionConfigOption;

use super::acpx_state::SessionAcpxState;
use super::model_state::apply_config_options_model_state;
use super::record::SessionRecord;

/// Ports `applyConfigOptionsToState`.
pub fn apply_config_options_to_state(
    state: Option<&SessionAcpxState>,
    config_options: Vec<SessionConfigOption>,
) -> SessionAcpxState {
    let mut acpx_state = state.cloned().unwrap_or_default();
    apply_config_options_model_state(&mut acpx_state, config_options);
    acpx_state
}

/// Ports `applyConfigOptionsToRecord`.
pub fn apply_config_options_to_record(
    record: &mut SessionRecord,
    config_options: Option<Vec<SessionConfigOption>>,
) {
    let Some(config_options) = config_options else {
        return;
    };
    record.acpx = Some(apply_config_options_to_state(
        record.acpx.as_ref(),
        config_options,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        SessionConfigId, SessionConfigKind, SessionConfigSelect, SessionConfigSelectOption,
    };

    fn model_option() -> SessionConfigOption {
        SessionConfigOption::new(
            SessionConfigId::new("model"),
            "Model",
            SessionConfigKind::Select(SessionConfigSelect::new(
                "m1",
                vec![SessionConfigSelectOption::new("m1", "m1")],
            )),
        )
    }

    #[test]
    fn none_leaves_state_untouched() {
        let state = apply_config_options_to_state(None, vec![]);
        assert!(state.config_options.as_ref().unwrap().is_empty());
    }

    #[test]
    fn applies_model_config_option_onto_fresh_state() {
        let state = apply_config_options_to_state(None, vec![model_option()]);
        assert_eq!(state.current_model_id.as_deref(), Some("m1"));
    }
}
