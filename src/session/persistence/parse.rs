//! Forward/backward-compat parsing of an on-disk session record.
//!
//! Ports `others/acpx/src/session/persistence/parse.ts` (878 lines of
//! manual field-by-field validation in TS). Per ADR-5, `serde`'s
//! `#[serde(default)]` (backward-compat: missing optional fields get sane
//! defaults) and `#[serde(flatten)] extra` (forward-compat: unknown fields
//! round-trip instead of erroring) make almost all of that file
//! structurally redundant — see the ADR's "why this over alternatives" for
//! the full reasoning. What's left, and what this module does, is exactly
//! the two-pass "sniff the schema tag first, then deserialize into the
//! matching versioned struct" dispatch the ADR calls for, so an
//! unrecognized future schema version is rejected explicitly instead of
//! attempting (and likely failing generically, or worse, silently
//! misparsing) a structural deserialize into today's [`SessionRecord`].

use serde_json::Value;

use crate::session::model_state::backfill_parsed_model_control;
use crate::session::record::SessionRecord;
use crate::session::schema::SessionSchemaVersion;

/// Ports `parseSessionRecord`. Returns `None` for anything that isn't a
/// JSON object, has an unrecognized/missing `schema` tag, or fails to
/// deserialize into [`SessionRecord`] — mirroring acpx's "return null on any
/// structural problem" contract (callers fall back to other resolution
/// strategies, e.g. `resolveSessionRecord`'s index-based search).
pub fn parse_session_record(raw: &Value) -> Option<SessionRecord> {
    let schema_tag = raw.as_object()?.get("schema")?.as_str()?;
    if schema_tag != SessionSchemaVersion::V1.as_str() {
        return None;
    }
    let mut record: SessionRecord = serde_json::from_value(raw.clone()).ok()?;
    // Ports `assignParsedModelState`'s tail backfill (gap 30): mutate the
    // parsed record in place so a missing `model_control` (with
    // `available_models` present) survives a parse -> serialize -> parse
    // round-trip, matching acpx's mutate-on-parse semantics.
    if let Some(acpx) = record.acpx.as_mut() {
        backfill_parsed_model_control(acpx);
    }
    Some(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::serialize::serialize_session_record_for_disk;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn round_trips_a_full_record() {
        let record = sample_session_record();
        let value = serialize_session_record_for_disk(&record);
        let parsed = parse_session_record(&value).expect("valid record should parse");
        // agent_session_id is normalized (trimmed) on serialize, so compare
        // against the serialized+reparsed record rather than the original.
        assert_eq!(parsed.acpx_record_id, record.acpx_record_id);
        assert_eq!(parsed.messages, record.messages);
        assert_eq!(parsed.agent_session_id.as_deref(), Some("agent-session-1"));
    }

    #[test]
    fn rejects_unrecognized_schema_tag() {
        let mut value = serialize_session_record_for_disk(&sample_session_record());
        value["schema"] = serde_json::json!("acpx.session.v1");
        assert!(parse_session_record(&value).is_none());
    }

    #[test]
    fn rejects_missing_schema_tag() {
        let mut value = serialize_session_record_for_disk(&sample_session_record());
        value.as_object_mut().unwrap().remove("schema");
        assert!(parse_session_record(&value).is_none());
    }

    #[test]
    fn missing_optional_field_gets_documented_default() {
        let mut value = serialize_session_record_for_disk(&sample_session_record());
        value.as_object_mut().unwrap().remove("closed");
        let parsed = parse_session_record(&value).expect("valid record should parse");
        assert!(!parsed.closed, "missing `closed` should default to false");
    }

    #[test]
    fn unknown_nested_field_round_trips_via_extra() {
        let mut value = serialize_session_record_for_disk(&sample_session_record());
        value["future_v2_field"] = serde_json::json!({"nested": true});
        let parsed = parse_session_record(&value).expect("valid record should parse");
        assert_eq!(parsed.extra.get("future_v2_field").unwrap()["nested"], true);

        let rewritten = serialize_session_record_for_disk(&parsed);
        assert_eq!(rewritten["future_v2_field"]["nested"], true);
    }

    #[test]
    fn rejects_non_object_input() {
        assert!(parse_session_record(&serde_json::json!("not an object")).is_none());
        assert!(parse_session_record(&serde_json::json!(null)).is_none());
    }

    fn model_config_option() -> serde_json::Value {
        use agent_client_protocol::schema::v1::{
            SessionConfigId, SessionConfigKind, SessionConfigOption, SessionConfigSelect,
            SessionConfigSelectOption,
        };
        let option = SessionConfigOption::new(
            SessionConfigId::new("model"),
            "Model",
            SessionConfigKind::Select(SessionConfigSelect::new(
                "gpt-5",
                vec![SessionConfigSelectOption::new("gpt-5", "gpt-5")],
            )),
        );
        serde_json::to_value(&option).unwrap()
    }

    #[test]
    fn backfills_model_control_as_config_option_when_a_model_config_option_is_present() {
        use crate::session::acpx_state::ModelControl;

        let mut value = serialize_session_record_for_disk(&sample_session_record());
        value["acpx"]["available_models"] = serde_json::json!(["gpt-5"]);
        value["acpx"]["config_options"] = serde_json::json!([model_config_option()]);
        value["acpx"]
            .as_object_mut()
            .unwrap()
            .remove("model_control");

        let parsed = parse_session_record(&value).expect("valid record should parse");
        assert_eq!(
            parsed.acpx.unwrap().model_control,
            Some(ModelControl::ConfigOption),
            "a model-designated config option should backfill config_option"
        );
    }

    #[test]
    fn backfills_model_control_as_legacy_set_model_when_no_model_config_option_present() {
        use crate::session::acpx_state::ModelControl;

        let mut value = serialize_session_record_for_disk(&sample_session_record());
        value["acpx"]["available_models"] = serde_json::json!(["legacy-model"]);
        value["acpx"]
            .as_object_mut()
            .unwrap()
            .remove("config_options");
        value["acpx"]
            .as_object_mut()
            .unwrap()
            .remove("model_control");

        let parsed = parse_session_record(&value).expect("valid record should parse");
        assert_eq!(
            parsed.acpx.unwrap().model_control,
            Some(ModelControl::LegacySetModel),
            "no model-designated config option should backfill legacy_set_model"
        );
    }

    #[test]
    fn backfilled_model_control_persists_through_a_round_trip() {
        use crate::session::acpx_state::ModelControl;

        let mut value = serialize_session_record_for_disk(&sample_session_record());
        value["acpx"]["available_models"] = serde_json::json!(["legacy-model"]);
        value["acpx"]
            .as_object_mut()
            .unwrap()
            .remove("config_options");
        value["acpx"]
            .as_object_mut()
            .unwrap()
            .remove("model_control");

        let parsed = parse_session_record(&value).expect("valid record should parse");
        assert_eq!(
            parsed.acpx.as_ref().unwrap().model_control,
            Some(ModelControl::LegacySetModel)
        );

        // Re-serialize and re-parse: the backfilled value must have been
        // written back onto the record itself (mutate-on-parse), not just
        // reconstructed transiently — so it survives a second round-trip
        // even without re-deriving it.
        let reserialized = serialize_session_record_for_disk(&parsed);
        assert_eq!(reserialized["acpx"]["model_control"], "legacy_set_model");
        let reparsed = parse_session_record(&reserialized).expect("re-parse should succeed");
        assert_eq!(
            reparsed.acpx.unwrap().model_control,
            Some(ModelControl::LegacySetModel)
        );
    }

    #[test]
    fn does_not_overwrite_an_explicit_model_control() {
        use crate::session::acpx_state::ModelControl;

        let mut value = serialize_session_record_for_disk(&sample_session_record());
        value["acpx"]["available_models"] = serde_json::json!(["gpt-5"]);
        value["acpx"]["model_control"] = serde_json::json!("config_option");
        value["acpx"]
            .as_object_mut()
            .unwrap()
            .remove("config_options");

        let parsed = parse_session_record(&value).expect("valid record should parse");
        assert_eq!(
            parsed.acpx.unwrap().model_control,
            Some(ModelControl::ConfigOption),
            "an explicit model_control must not be overwritten by the backfill"
        );
    }
}
