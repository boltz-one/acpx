//! Desired mode/model/config-option get/set/sync helpers.
//!
//! Ports `others/acpx/src/session/mode-preference.ts`. "Desired" state is
//! what the user asked for (persisted across reconnects); "current" state
//! is what the agent last confirmed.

use std::collections::HashMap;

use crate::agent_command::model_support::SessionModelState;
use crate::session::acpx_state::SessionAcpxState;
use crate::session::model_state::apply_advertised_model_state;
use crate::session::record::SessionRecord;
use crate::types::PermissionMode;

fn normalize_trimmed(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Ports `normalizeModeId` (also reused by acpx for model ids via a
/// structurally identical private `normalizeModelId`).
pub fn normalize_mode_id(mode_id: Option<&str>) -> Option<String> {
    normalize_trimmed(mode_id)
}

fn ensure_acpx_state(state: Option<SessionAcpxState>) -> SessionAcpxState {
    state.unwrap_or_default()
}

/// Ports `getDesiredModeId`.
pub fn get_desired_mode_id(state: Option<&SessionAcpxState>) -> Option<String> {
    normalize_mode_id(state.and_then(|s| s.desired_mode_id.as_deref()))
}

/// Ports `getDesiredConfigOptions`.
pub fn get_desired_config_options(state: Option<&SessionAcpxState>) -> HashMap<String, String> {
    let Some(desired) = state.and_then(|s| s.desired_config_options.as_ref()) else {
        return HashMap::new();
    };
    desired
        .iter()
        .filter_map(|(config_id, value)| {
            normalize_mode_id(Some(config_id)).map(|normalized| (normalized, value.clone()))
        })
        .collect()
}

/// Ports `setDesiredModeId`.
pub fn set_desired_mode_id(record: &mut SessionRecord, mode_id: Option<&str>) {
    let mut acpx = ensure_acpx_state(record.acpx.take());
    acpx.desired_mode_id = normalize_mode_id(mode_id);
    record.acpx = Some(acpx);
}

/// Ports `setDesiredConfigOption`.
pub fn set_desired_config_option(record: &mut SessionRecord, config_id: &str, value: Option<&str>) {
    let Some(normalized_config_id) = normalize_mode_id(Some(config_id)) else {
        return;
    };
    if normalized_config_id == "mode" || normalized_config_id == "model" {
        return;
    }

    let mut acpx = ensure_acpx_state(record.acpx.take());
    let mut desired = acpx.desired_config_options.clone().unwrap_or_default();
    match value {
        Some(value) => {
            desired.insert(normalized_config_id, value.to_string());
        }
        None => {
            desired.remove(&normalized_config_id);
        }
    }
    acpx.desired_config_options = (!desired.is_empty()).then_some(desired);
    record.acpx = Some(acpx);
}

/// Persists the host-set live permission mode (Write/Ask) so a reconnect
/// re-applies it. Distinct from the ACP `mode` config option.
pub fn set_desired_permission_mode(record: &mut SessionRecord, mode: PermissionMode) {
    let mut acpx = ensure_acpx_state(record.acpx.take());
    acpx.desired_permission_mode = Some(mode);
    record.acpx = Some(acpx);
}

/// Reads the persisted desired permission mode (for reconnect re-application).
pub fn get_desired_permission_mode(state: Option<&SessionAcpxState>) -> Option<PermissionMode> {
    state.and_then(|s| s.desired_permission_mode)
}

/// Ports `clearDesiredConfigOption`.
pub fn clear_desired_config_option(state: &mut SessionAcpxState, config_id: Option<&str>) {
    let Some(normalized_config_id) = normalize_mode_id(config_id) else {
        return;
    };
    let Some(desired) = &mut state.desired_config_options else {
        return;
    };
    desired.remove(&normalized_config_id);
    if desired.is_empty() {
        state.desired_config_options = None;
    }
}

/// Ports `getDesiredModelId`.
pub fn get_desired_model_id(state: Option<&SessionAcpxState>) -> Option<String> {
    normalize_mode_id(
        state
            .and_then(|s| s.session_options.as_ref())
            .and_then(|options| options.model.as_deref()),
    )
}

fn has_stored_session_options(options: &crate::session::acpx_state::SessionOptions) -> bool {
    options.model.is_some()
        || options.allowed_tools.is_some()
        || options.max_turns.is_some()
        || options.system_prompt.is_some()
        || options.env.is_some()
}

/// Ports `setDesiredModelId`.
pub fn set_desired_model_id(
    record: &mut SessionRecord,
    model_id: Option<&str>,
    model_config_id: Option<&str>,
) {
    let mut acpx = ensure_acpx_state(record.acpx.take());
    let mut session_options = acpx.session_options.clone().unwrap_or_default();
    session_options.model = normalize_mode_id(model_id);
    acpx.session_options = has_stored_session_options(&session_options).then_some(session_options);

    let model_config_id = model_config_id.map(str::to_string).or_else(|| {
        crate::agent_command::model_support::model_state_from_config_options(
            acpx.config_options.as_deref().unwrap_or(&[]),
        )
        .and_then(|models| models.config_id)
    });
    clear_desired_config_option(&mut acpx, model_config_id.as_deref());
    record.acpx = Some(acpx);
}

/// Ports `setCurrentModelId`.
pub fn set_current_model_id(record: &mut SessionRecord, model_id: Option<&str>) {
    let mut acpx = ensure_acpx_state(record.acpx.take());
    acpx.current_model_id = normalize_mode_id(model_id);
    record.acpx = Some(acpx);
}

/// Ports `syncAdvertisedModelState`.
pub fn sync_advertised_model_state(record: &mut SessionRecord, models: Option<&SessionModelState>) {
    let Some(models) = models else {
        return;
    };
    let mut acpx = ensure_acpx_state(record.acpx.take());
    apply_advertised_model_state(&mut acpx, models);
    record.acpx = Some(acpx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn set_and_get_desired_mode_id_round_trips() {
        let mut record = sample_session_record();
        set_desired_mode_id(&mut record, Some(" plan "));
        assert_eq!(
            get_desired_mode_id(record.acpx.as_ref()),
            Some("plan".to_string())
        );

        set_desired_mode_id(&mut record, None);
        assert_eq!(get_desired_mode_id(record.acpx.as_ref()), None);
    }

    #[test]
    fn set_desired_config_option_rejects_reserved_ids() {
        let mut record = sample_session_record();
        set_desired_config_option(&mut record, "mode", Some("x"));
        assert!(get_desired_config_options(record.acpx.as_ref()).is_empty());

        set_desired_config_option(&mut record, "temperature", Some("0.5"));
        assert_eq!(
            get_desired_config_options(record.acpx.as_ref()).get("temperature"),
            Some(&"0.5".to_string())
        );
    }

    #[test]
    fn set_desired_model_id_clears_matching_config_option() {
        let mut record = sample_session_record();
        // `setDesiredConfigOption` itself always rejects `"model"` as a
        // reserved id, so simulate a pre-existing "model" desired config
        // option the way an older/imported record might carry one, and
        // confirm `setDesiredModelId` clears it via `model_config_id`.
        let mut acpx = record.acpx.take().unwrap();
        acpx.desired_config_options = Some(HashMap::from([(
            "model".to_string(),
            "ignored".to_string(),
        )]));
        record.acpx = Some(acpx);

        set_desired_model_id(&mut record, Some("gpt-5"), Some("model"));
        assert_eq!(
            get_desired_model_id(record.acpx.as_ref()),
            Some("gpt-5".to_string())
        );
        assert!(!get_desired_config_options(record.acpx.as_ref()).contains_key("model"));
    }

    #[test]
    fn set_and_get_desired_permission_mode_round_trips() {
        let mut record = sample_session_record();
        set_desired_permission_mode(&mut record, PermissionMode::DenyAll);
        assert_eq!(
            get_desired_permission_mode(record.acpx.as_ref()),
            Some(PermissionMode::DenyAll)
        );

        let serialized = serde_json::to_string(record.acpx.as_ref().unwrap()).unwrap();
        assert!(
            serialized.contains("\"desired_permission_mode\":\"deny-all\""),
            "persists kebab-case: {serialized}"
        );
    }
}
