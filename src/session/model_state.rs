//! Session-level "advertised model" bookkeeping.
//!
//! Ports `others/acpx/src/session/model-state.ts`. Note this is a different
//! concept from [`crate::agent_command::model_support::SessionModelState`]
//! (which this module wraps): that type is the *ephemeral* model state an
//! agent just advertised over the wire; this module folds it into the
//! *persisted* [`SessionAcpxState`].

use agent_client_protocol::schema::v1::{SessionConfigOption, SessionConfigOptionCategory};

use crate::agent_command::model_support::{SessionModelState, model_state_from_config_options};
use crate::session::acpx_state::{ModelControl, SessionAcpxState};
use crate::session::record::SessionRecord;

fn config_options_are_authoritative(state: &SessionAcpxState) -> bool {
    state.model_control == Some(ModelControl::ConfigOption)
}

fn legacy_model_state(state: &SessionAcpxState) -> Option<SessionModelState> {
    let available_models = state.available_models.as_ref()?;
    Some(SessionModelState {
        config_id: None,
        current_model_id: state.current_model_id.clone().unwrap_or_default(),
        available_models: available_models
            .iter()
            .map(
                |model_id| crate::agent_command::model_support::AvailableModel {
                    model_id: model_id.clone(),
                    name: model_id.clone(),
                },
            )
            .collect(),
    })
}

/// Ports `advertisedModelState`.
pub fn advertised_model_state(state: Option<&SessionAcpxState>) -> Option<SessionModelState> {
    let state = state?;
    let config_options = state.config_options.as_deref().unwrap_or(&[]);
    if let Some(models) = model_state_from_config_options(config_options) {
        return Some(models);
    }
    if config_options_are_authoritative(state) {
        return None;
    }
    legacy_model_state(state)
}

/// Ports `applyAdvertisedModelState`.
pub fn apply_advertised_model_state(state: &mut SessionAcpxState, models: &SessionModelState) {
    state.current_model_id = Some(models.current_model_id.clone());
    state.available_models = Some(
        models
            .available_models
            .iter()
            .map(|m| m.model_id.clone())
            .collect(),
    );
    state.model_control = Some(if models.config_id.is_some() {
        ModelControl::ConfigOption
    } else {
        ModelControl::LegacySetModel
    });
}

/// Ports `clearAdvertisedModelState`.
pub fn clear_advertised_model_state(state: &mut SessionAcpxState) {
    state.current_model_id = None;
    state.available_models = None;
    state.model_control = None;
}

fn is_model_config_option(option: &SessionConfigOption) -> bool {
    option.category == Some(SessionConfigOptionCategory::Model) || option.id.0.as_ref() == "model"
}

/// Ports `hasModelConfigOption` (`parse.ts`): does `config_options` contain
/// at least one model-designated option (matching [`is_model_config_option`]
/// exactly)?
fn has_model_config_option(config_options: Option<&[SessionConfigOption]>) -> bool {
    config_options
        .unwrap_or(&[])
        .iter()
        .any(is_model_config_option)
}

/// Ports `assignParsedModelState`'s tail backfill (gap 30): when a parsed
/// record has `available_models` but no explicit `model_control`, acpx
/// mutates the parsed state in place to backfill it — `config_option` if a
/// model-designated config option is present, else `legacy_set_model` —
/// so the backfilled value survives re-serialization (unlike this crate's
/// [`advertised_model_state`]/[`legacy_model_state`], which only reconstruct
/// the equivalent view on demand at read time and never persist it).
///
/// Called once at parse time (`persistence/parse.rs`), not at every read, to
/// mirror acpx's "mutate on parse" semantics exactly.
pub fn backfill_parsed_model_control(state: &mut SessionAcpxState) {
    if state.model_control.is_some() || state.available_models.is_none() {
        return;
    }
    state.model_control = Some(
        if has_model_config_option(state.config_options.as_deref()) {
            ModelControl::ConfigOption
        } else {
            ModelControl::LegacySetModel
        },
    );
}

/// Ports `removeModelConfigOptions`.
pub fn remove_model_config_options(state: &mut SessionAcpxState) {
    if let Some(config_options) = &mut state.config_options {
        config_options.retain(|option| !is_model_config_option(option));
    }
}

/// Ports `applyConfigOptionsModelState`.
pub fn apply_config_options_model_state(
    state: &mut SessionAcpxState,
    config_options: Vec<SessionConfigOption>,
) {
    let previous_config_models =
        model_state_from_config_options(state.config_options.as_deref().unwrap_or(&[]));
    let preserves_legacy_control = state.model_control == Some(ModelControl::LegacySetModel)
        || (state.model_control.is_none()
            && previous_config_models.is_none()
            && legacy_model_state(state).is_some());

    let models = model_state_from_config_options(&config_options);
    state.config_options = Some(config_options);

    if let Some(models) = &models {
        apply_advertised_model_state(state, models);
    } else if preserves_legacy_control {
        state.model_control = Some(ModelControl::LegacySetModel);
    } else {
        clear_advertised_model_state(state);
    }
}

/// Ports `clearOmittedFreshSessionConfigOptions` (gap 15): a freshly created
/// backend session (`session/new`) whose response omitted `configOptions`
/// entirely must not leave a *stale* `config_options` value from an earlier
/// connection attempt lying around on the record — that stale value would
/// otherwise be misread as "this fresh session still advertises these
/// options" by any later `advertised_model_state`/status call.
pub fn clear_omitted_fresh_session_config_options(
    record: &mut SessionRecord,
    created_fresh_session: bool,
    config_options_present: bool,
) {
    if !created_fresh_session || config_options_present {
        return;
    }
    if let Some(acpx) = record.acpx.as_mut() {
        acpx.config_options = None;
    }
}

/// Ports `syncAdvertisedModelState`: folds a freshly observed
/// [`SessionModelState`] (from a `session/new`/`session/load`/
/// `session/resume` response, already merging the config-option and legacy
/// `_meta.models` shapes via
/// [`crate::agent_command::model_support::model_state_from_session_response`])
/// onto the record's persisted `acpx` state. A no-op when `models` is
/// `None` — there is nothing new to sync.
pub fn sync_advertised_model_state(record: &mut SessionRecord, models: Option<&SessionModelState>) {
    let Some(models) = models else {
        return;
    };
    let mut acpx = record.acpx.take().unwrap_or_default();
    apply_advertised_model_state(&mut acpx, models);
    record.acpx = Some(acpx);
}

/// Ports `applyReconnectedModelState`: the single reconciliation point
/// `connect_and_load_session`'s tail calls after any acquisition path
/// (create-fresh, resume, or load), regardless of whether preference replay
/// ran. Mirrors acpx's `reconnect.ts` function of the same name exactly:
///
/// - Clears a stale `config_options` value when a freshly created session's
///   response omitted the field (Requirement 5).
/// - When `models` is present: if the response carried *legacy*
///   `_meta.models` metadata (`legacy_model_metadata_present`) with no
///   config-option-derived model (`models.config_id.is_none()`), strips any
///   leftover `model`-category config options so the legacy state is
///   authoritative (`remove_model_config_options`), then syncs `models` onto
///   the record.
/// - When `models` is absent: clears the record's advertised model state
///   entirely, but only when that absence is itself meaningful — either the
///   response explicitly carried legacy metadata (which was empty/invalid)
///   or this is a freshly created session (which cannot have carried over
///   model state from a previous backend session id).
pub fn apply_reconnected_model_state(
    record: &mut SessionRecord,
    models: Option<&SessionModelState>,
    config_options_present: bool,
    legacy_model_metadata_present: bool,
    created_fresh_session: bool,
) {
    clear_omitted_fresh_session_config_options(
        record,
        created_fresh_session,
        config_options_present,
    );

    match models {
        Some(models) => {
            if legacy_model_metadata_present && models.config_id.is_none() {
                if let Some(acpx) = record.acpx.as_mut() {
                    remove_model_config_options(acpx);
                }
            }
            sync_advertised_model_state(record, Some(models));
        }
        None => {
            let should_clear = legacy_model_metadata_present || created_fresh_session;
            if should_clear {
                if let Some(acpx) = record.acpx.as_mut() {
                    clear_advertised_model_state(acpx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        SessionConfigId, SessionConfigKind, SessionConfigSelect, SessionConfigSelectOption,
    };

    fn model_option(current: &'static str) -> SessionConfigOption {
        SessionConfigOption::new(
            SessionConfigId::new("model"),
            "Model",
            SessionConfigKind::Select(SessionConfigSelect::new(
                current,
                vec![SessionConfigSelectOption::new(
                    "default-model",
                    "default-model",
                )],
            )),
        )
    }

    #[test]
    fn applying_config_options_sets_advertised_model_state() {
        let mut state = SessionAcpxState::default();
        apply_config_options_model_state(&mut state, vec![model_option("default-model")]);
        assert_eq!(state.current_model_id.as_deref(), Some("default-model"));
        assert_eq!(state.model_control, Some(ModelControl::ConfigOption));
    }

    #[test]
    fn legacy_state_used_when_no_config_option_models_advertised() {
        let mut state = SessionAcpxState {
            current_model_id: Some("legacy-model".into()),
            available_models: Some(vec!["legacy-model".into()]),
            ..Default::default()
        };
        let models = advertised_model_state(Some(&state)).unwrap();
        assert_eq!(models.current_model_id, "legacy-model");

        clear_advertised_model_state(&mut state);
        assert!(state.current_model_id.is_none());
    }

    fn sample_record_with_config_options() -> SessionRecord {
        let mut record =
            crate::session::persistence::serialize::test_support::sample_session_record();
        record.acpx = Some(SessionAcpxState {
            config_options: Some(vec![model_option("stale-model")]),
            current_model_id: Some("stale-model".into()),
            model_control: Some(ModelControl::ConfigOption),
            ..Default::default()
        });
        record
    }

    #[test]
    fn clear_omitted_fresh_session_config_options_clears_stale_value() {
        let mut record = sample_record_with_config_options();
        clear_omitted_fresh_session_config_options(&mut record, true, false);
        assert!(record.acpx.unwrap().config_options.is_none());
    }

    #[test]
    fn clear_omitted_fresh_session_config_options_keeps_value_when_present() {
        let mut record = sample_record_with_config_options();
        clear_omitted_fresh_session_config_options(&mut record, true, true);
        assert!(record.acpx.unwrap().config_options.is_some());
    }

    #[test]
    fn clear_omitted_fresh_session_config_options_ignores_non_fresh_sessions() {
        let mut record = sample_record_with_config_options();
        clear_omitted_fresh_session_config_options(&mut record, false, false);
        assert!(
            record.acpx.unwrap().config_options.is_some(),
            "only a freshly created session's omitted config_options should be cleared"
        );
    }

    #[test]
    fn sync_advertised_model_state_applies_models_onto_record() {
        let mut record =
            crate::session::persistence::serialize::test_support::sample_session_record();
        let models = SessionModelState {
            config_id: Some("model".into()),
            current_model_id: "gpt-5".into(),
            available_models: vec![],
        };
        sync_advertised_model_state(&mut record, Some(&models));
        assert_eq!(
            record.acpx.unwrap().current_model_id.as_deref(),
            Some("gpt-5")
        );
    }

    #[test]
    fn sync_advertised_model_state_is_a_no_op_for_none() {
        let mut record =
            crate::session::persistence::serialize::test_support::sample_session_record();
        let before = record.acpx.clone();
        sync_advertised_model_state(&mut record, None);
        assert_eq!(
            record.acpx, before,
            "None must leave existing acpx state untouched"
        );
    }

    #[test]
    fn apply_reconnected_model_state_syncs_legacy_models_and_strips_model_config_options() {
        let mut record = sample_record_with_config_options();
        let legacy_models = SessionModelState {
            config_id: None,
            current_model_id: "legacy-model".into(),
            available_models: vec![],
        };
        apply_reconnected_model_state(&mut record, Some(&legacy_models), true, true, false);
        let acpx = record.acpx.unwrap();
        assert_eq!(acpx.current_model_id.as_deref(), Some("legacy-model"));
        assert_eq!(acpx.model_control, Some(ModelControl::LegacySetModel));
        assert!(
            acpx.config_options.as_ref().unwrap().is_empty(),
            "legacy-authoritative model state should strip stale model config options"
        );
    }

    #[test]
    fn apply_reconnected_model_state_clears_state_for_fresh_session_with_no_models() {
        let mut record = sample_record_with_config_options();
        apply_reconnected_model_state(&mut record, None, false, false, true);
        let acpx = record.acpx.unwrap();
        assert!(acpx.current_model_id.is_none());
        assert!(acpx.config_options.is_none());
    }

    #[test]
    fn apply_reconnected_model_state_preserves_state_when_nothing_changed() {
        let mut record = sample_record_with_config_options();
        // Not a fresh session, no legacy metadata, no new models observed:
        // a plain resume with an unrelated response must not wipe out
        // previously-known model state.
        apply_reconnected_model_state(&mut record, None, false, false, false);
        let acpx = record.acpx.unwrap();
        assert_eq!(acpx.current_model_id.as_deref(), Some("stale-model"));
    }
}
