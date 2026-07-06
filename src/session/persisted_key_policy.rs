//! Persisted-key-policy regression check.
//!
//! Ports `others/acpx/src/persisted-key-policy.ts`. In acpx this runs on
//! every write as a runtime safety net: TS objects default to camelCase
//! JSON, but the persisted format is deliberately snake_case, so the check
//! guards against a future contributor forgetting to convert a new field.
//!
//! Per ADR-5 (`plans/20260705-1718-acpx-to-acp-crate-port/phase-05-session-persistence.md`),
//! that entire bug class doesn't exist in the Rust port *by construction*:
//! `serde` serializes struct field names as written, and this crate's
//! [`super::record::SessionRecord`] fields are already snake_case. This
//! module therefore isn't wired into the write path in release builds (see
//! [`super::persistence::repository::write_session_record`], which calls it
//! only `#[cfg(debug_assertions)]`) — it exists as a regression test against
//! an accidental future `#[serde(rename_all = "camelCase")]` addition, not
//! a hot-path assertion.

use serde_json::Value;

/// acpx's internally-tagged enum discriminants (`SessionMessage`/
/// `SessionUserContent`/`SessionAgentContent`/`SessionToolResultContent`
/// variants) are deliberately PascalCase amid an otherwise-snake_case
/// document.
const TAG_KEYS: &[&str] = &[
    "User",
    "Agent",
    "Resume",
    "Text",
    "Mention",
    "Image",
    "Audio",
    "Thinking",
    "RedactedThinking",
    "ToolUse",
];

/// Paths whose *own* key may be non-snake_case because the object at that
/// path is a map keyed by caller-controlled ids (tool-call ids, request
/// ids), not a struct with named fields.
const MAP_OBJECT_PATHS: &[&str] = &["request_token_usage", "messages.Agent.tool_results"];

/// Paths whose *value* is opaque (raw wire payloads) and must not be
/// descended into at all.
const OPAQUE_VALUE_PATHS: &[&str] = &[
    "agent_capabilities",
    "messages.Agent.content.ToolUse.input",
    "acpx.desired_config_options",
    "acpx.config_options",
    // `SessionOptions.env` (Phase 4's `session_options` persistence, see
    // `runtime::engine::session_options`) is a `HashMap<String, String>` of
    // caller-chosen environment variable names — legitimately
    // SCREAMING_SNAKE_CASE or otherwise non-lowercase-snake_case, same
    // reasoning as `desired_config_options` above.
    "acpx.session_options.env",
];

fn is_snake_case_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn join_path(path: &[String]) -> String {
    path.join(".")
}

fn should_skip_key_rule(path: &[String]) -> bool {
    MAP_OBJECT_PATHS.contains(&join_path(path).as_str())
}

fn is_tool_result_output_path(path: &[String]) -> bool {
    if path.len() < 5 || path.last().map(String::as_str) != Some("output") {
        return false;
    }
    let Some(tool_results_index) = path.iter().rposition(|segment| segment == "tool_results")
    else {
        return false;
    };
    if tool_results_index + 2 != path.len() - 1 {
        return false;
    }
    join_path(&path[..=tool_results_index]) == "messages.Agent.tool_results"
}

fn should_skip_descend(path: &[String]) -> bool {
    OPAQUE_VALUE_PATHS.contains(&join_path(path).as_str()) || is_tool_result_output_path(path)
}

fn collect_violations(value: &Value, path: &[String], violations: &mut Vec<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_violations(item, path, violations);
            }
        }
        Value::Object(map) => {
            let skip_key_rule = should_skip_key_rule(path);
            for (key, child) in map {
                if !skip_key_rule && !is_snake_case_key(key) && !TAG_KEYS.contains(&key.as_str()) {
                    let mut violation_path = path.to_vec();
                    violation_path.push(key.clone());
                    violations.push(join_path(&violation_path));
                }

                let mut child_path = path.to_vec();
                child_path.push(key.clone());
                if !should_skip_descend(&child_path) {
                    collect_violations(child, &child_path, violations);
                }
            }
        }
        _ => {}
    }
}

/// Ports `findPersistedKeyPolicyViolations`.
pub fn find_persisted_key_policy_violations(value: &Value) -> Vec<String> {
    let mut violations = Vec::new();
    collect_violations(value, &[], &mut violations);
    violations
}

/// Ports `assertPersistedKeyPolicy`.
pub fn assert_persisted_key_policy(value: &Value) -> Result<(), String> {
    let violations = find_persisted_key_policy_violations(value);
    if violations.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Persisted key policy violation (expected snake_case keys): {}",
        violations.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_camel_case_key() {
        let value = serde_json::json!({"lastUsedAt": "now"});
        assert_eq!(
            find_persisted_key_policy_violations(&value),
            vec!["lastUsedAt".to_string()]
        );
    }

    #[test]
    fn allows_pascal_case_tag_keys() {
        let value = serde_json::json!({"messages": [{"User": {"id": "1", "content": []}}]});
        assert!(find_persisted_key_policy_violations(&value).is_empty());
    }

    #[test]
    fn skips_opaque_agent_capabilities_blob() {
        let value = serde_json::json!({"agent_capabilities": {"loadSession": true}});
        assert!(find_persisted_key_policy_violations(&value).is_empty());
    }

    #[test]
    fn skips_tool_results_map_keys_but_checks_their_fields() {
        let value = serde_json::json!({
            "messages": [{
                "Agent": {
                    "content": [],
                    "tool_results": {
                        "call-123": {"tool_use_id": "call-123", "badKey": 1}
                    }
                }
            }]
        });
        let violations = find_persisted_key_policy_violations(&value);
        assert_eq!(
            violations,
            vec!["messages.Agent.tool_results.call-123.badKey".to_string()]
        );
    }

    /// Regression test per ADR-5: a full [`super::super::record::SessionRecord`]
    /// serializes with zero policy violations. This is the debug-only
    /// analog of acpx's runtime `assertPersistedKeyPolicy` call — it exists
    /// to catch an accidental future `#[serde(rename_all = "camelCase")]`
    /// on a persisted struct, not to run on every write.
    #[test]
    fn full_session_record_has_no_persisted_key_policy_violations() {
        let record = crate::session::persistence::serialize::test_support::sample_session_record();
        let value =
            crate::session::persistence::serialize::serialize_session_record_for_disk(&record);
        assert_eq!(
            find_persisted_key_policy_violations(&value),
            Vec::<String>::new()
        );
    }
}
