//! Ports `others/acpx/src/runtime/engine/session-options.ts`: the live
//! per-session agent options (`SessionAgentOptions`) applied when a fresh
//! ACP session is created, and their persisted form on
//! [`crate::session::acpx_state::SessionOptions`].

use std::collections::HashMap;

use agent_client_protocol::schema::v1::Meta;
use serde_json::Value;

use crate::session::acpx_state::{SessionOptions, SystemPromptOption};
use crate::session::record::SessionRecord;

/// Ports `CLAUDE_CODE_DEFAULT_SETTING_SOURCES`.
const CLAUDE_CODE_DEFAULT_SETTING_SOURCES: [&str; 2] = ["project", "local"];

/// Ports `resolveClaudeCodeSettingSources`. Kept as `ACPX_CLAUDE_*` (not
/// renamed to this crate's usual `ACP_` prefix) to match the existing
/// convention `agent_command::claude_quirks`/`gemini_quirks` already
/// established for acpx-ported env-var names in this crate.
pub fn resolve_claude_code_setting_sources() -> Vec<String> {
    let include_user_settings = std::env::var("ACPX_CLAUDE_INCLUDE_USER_SETTINGS")
        .map(|value| value.trim() == "1")
        .unwrap_or(false);
    let mut sources = Vec::with_capacity(3);
    if include_user_settings {
        sources.push("user".to_string());
    }
    sources.extend(
        CLAUDE_CODE_DEFAULT_SETTING_SOURCES
            .iter()
            .map(|s| s.to_string()),
    );
    sources
}

/// Ports `buildClaudeCodeOptionsMeta`: builds the `_meta.claudeCode.options`
/// (plus a top-level `_meta.systemPrompt`, matching acpx's
/// `assignClaudeCodeSystemPrompt` writing onto `meta` rather than the nested
/// `claudeCode.options` object) block a fresh `session/new` request attaches
/// so a Claude Code ACP adapter picks up the session's persisted
/// model/allowedTools/maxTurns/systemPrompt. Applied unconditionally
/// (Requirement 2) — non-Claude agents ignore unrecognized `_meta` keys per
/// ACP's extensibility convention, so there's no need to gate this on
/// `is_claude_acp`. `isolate_user_settings` (acpx's `isolateUserSettings`
/// parameter) IS still gated by the caller on `is_claude_acp`, matching
/// acpx's own call site (`client.ts`'s `createSession`).
pub fn build_claude_code_options_meta(
    options: Option<&SessionAgentOptions>,
    isolate_user_settings: bool,
) -> Option<Meta> {
    let mut claude_code_options = serde_json::Map::new();
    if isolate_user_settings {
        claude_code_options.insert(
            "settingSources".to_string(),
            Value::from(resolve_claude_code_setting_sources()),
        );
    }
    if let Some(options) = options {
        if let Some(model) = options.model.as_deref() {
            if !model.trim().is_empty() {
                claude_code_options.insert("model".to_string(), Value::from(model));
            }
        }
        if let Some(allowed_tools) = &options.allowed_tools {
            claude_code_options.insert(
                "allowedTools".to_string(),
                Value::from(allowed_tools.clone()),
            );
        }
        if let Some(max_turns) = options.max_turns {
            claude_code_options.insert("maxTurns".to_string(), Value::from(max_turns));
        }
    }

    let mut meta = serde_json::Map::new();
    if !claude_code_options.is_empty() {
        meta.insert(
            "claudeCode".to_string(),
            serde_json::json!({ "options": claude_code_options }),
        );
    }

    if let Some(system_prompt) = options.and_then(|o| o.system_prompt.as_ref()) {
        let has_content = match system_prompt {
            SystemPromptOption::Direct(text) => !text.is_empty(),
            SystemPromptOption::Append { append } => !append.is_empty(),
        };
        if has_content {
            meta.insert(
                "systemPrompt".to_string(),
                serde_json::to_value(system_prompt).expect("SystemPromptOption always serializes"),
            );
        }
    }

    (!meta.is_empty()).then_some(meta)
}

/// Ports `SessionAgentOptions`. Threaded into a fresh `session/new` request
/// (system prompt / env) and persisted onto the new record so a later
/// reconnect can recreate an equivalent session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionAgentOptions {
    pub model: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub max_turns: Option<u32>,
    pub system_prompt: Option<SystemPromptOption>,
    pub env: Option<HashMap<String, String>>,
}

fn merge_env(
    fallback: Option<&HashMap<String, String>>,
    preferred: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    if fallback.is_none() && preferred.is_none() {
        return None;
    }
    let mut merged = fallback.cloned().unwrap_or_default();
    if let Some(preferred) = preferred {
        merged.extend(preferred.clone());
    }
    Some(merged)
}

/// Ports `mergeSessionOptions`: `preferred` fields win over `fallback`,
/// `env` maps are shallow-merged (`preferred` wins per key).
pub fn merge_session_options(
    preferred: Option<&SessionAgentOptions>,
    fallback: Option<&SessionAgentOptions>,
) -> Option<SessionAgentOptions> {
    let merged = SessionAgentOptions {
        model: preferred
            .and_then(|p| p.model.clone())
            .or_else(|| fallback.and_then(|f| f.model.clone())),
        allowed_tools: preferred
            .and_then(|p| p.allowed_tools.clone())
            .or_else(|| fallback.and_then(|f| f.allowed_tools.clone())),
        max_turns: preferred
            .and_then(|p| p.max_turns)
            .or_else(|| fallback.and_then(|f| f.max_turns)),
        system_prompt: preferred
            .and_then(|p| p.system_prompt.clone())
            .or_else(|| fallback.and_then(|f| f.system_prompt.clone())),
        env: merge_env(
            fallback.and_then(|f| f.env.as_ref()),
            preferred.and_then(|p| p.env.as_ref()),
        ),
    };
    has_any_option(&merged).then_some(merged)
}

fn has_any_option(options: &SessionAgentOptions) -> bool {
    options.model.is_some()
        || options.allowed_tools.is_some()
        || options.max_turns.is_some()
        || options.system_prompt.is_some()
        || options.env.is_some()
}

fn persisted_session_options(options: &SessionAgentOptions) -> Option<SessionOptions> {
    let persisted = SessionOptions {
        model: options.model.clone(),
        allowed_tools: options.allowed_tools.clone(),
        max_turns: options.max_turns,
        system_prompt: options.system_prompt.clone(),
        env: options.env.clone(),
    };
    has_stored_session_options(&persisted).then_some(persisted)
}

fn has_stored_session_options(options: &SessionOptions) -> bool {
    options.model.is_some()
        || options.allowed_tools.is_some()
        || options.max_turns.is_some()
        || options.system_prompt.is_some()
        || options.env.is_some()
}

/// Ports `persistSessionOptions`.
pub fn persist_session_options(record: &mut SessionRecord, options: Option<&SessionAgentOptions>) {
    let next = options.and_then(persisted_session_options);
    if let Some(next) = next {
        let mut acpx = record.acpx.take().unwrap_or_default();
        acpx.session_options = Some(next);
        record.acpx = Some(acpx);
        return;
    }

    if let Some(acpx) = &mut record.acpx {
        acpx.session_options = None;
    }
}

/// Ports `sessionOptionsFromRecord`.
pub fn session_options_from_record(record: &SessionRecord) -> Option<SessionAgentOptions> {
    let stored = record.acpx.as_ref()?.session_options.as_ref()?;
    let options = SessionAgentOptions {
        model: stored.model.clone(),
        allowed_tools: stored.allowed_tools.clone(),
        max_turns: stored.max_turns,
        system_prompt: stored.system_prompt.clone(),
        env: stored.env.clone(),
    };
    has_any_option(&options).then_some(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn merge_prefers_preferred_over_fallback() {
        let fallback = SessionAgentOptions {
            model: Some("fallback-model".into()),
            max_turns: Some(5),
            ..Default::default()
        };
        let preferred = SessionAgentOptions {
            model: Some("preferred-model".into()),
            ..Default::default()
        };
        let merged = merge_session_options(Some(&preferred), Some(&fallback)).unwrap();
        assert_eq!(merged.model.as_deref(), Some("preferred-model"));
        assert_eq!(merged.max_turns, Some(5));
    }

    #[test]
    fn persist_and_read_round_trips() {
        let mut record = sample_session_record();
        let options = SessionAgentOptions {
            model: Some("gpt-5".into()),
            ..Default::default()
        };
        persist_session_options(&mut record, Some(&options));
        let read_back = session_options_from_record(&record).unwrap();
        assert_eq!(read_back.model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn persist_none_clears_existing_options() {
        let mut record = sample_session_record();
        let options = SessionAgentOptions {
            model: Some("gpt-5".into()),
            ..Default::default()
        };
        persist_session_options(&mut record, Some(&options));
        persist_session_options(&mut record, None);
        assert!(session_options_from_record(&record).is_none());
    }

    #[test]
    fn no_options_and_no_isolation_yields_no_meta() {
        assert_eq!(build_claude_code_options_meta(None, false), None);
    }

    #[test]
    fn model_and_max_turns_map_into_claude_code_options() {
        let options = SessionAgentOptions {
            model: Some("gpt-5".into()),
            max_turns: Some(3),
            allowed_tools: Some(vec!["bash".into()]),
            ..Default::default()
        };
        let meta = build_claude_code_options_meta(Some(&options), false).unwrap();
        let claude_code = &meta["claudeCode"]["options"];
        assert_eq!(claude_code["model"], "gpt-5");
        assert_eq!(claude_code["maxTurns"], 3);
        assert_eq!(claude_code["allowedTools"][0], "bash");
        assert!(meta.get("systemPrompt").is_none());
    }

    #[test]
    fn direct_system_prompt_is_a_plain_string_at_top_level() {
        let options = SessionAgentOptions {
            system_prompt: Some(SystemPromptOption::Direct("be terse".into())),
            ..Default::default()
        };
        let meta = build_claude_code_options_meta(Some(&options), false).unwrap();
        assert_eq!(meta["systemPrompt"], "be terse");
        assert!(meta.get("claudeCode").is_none());
    }

    #[test]
    fn append_system_prompt_is_an_object_at_top_level() {
        let options = SessionAgentOptions {
            system_prompt: Some(SystemPromptOption::Append {
                append: "and be nice".into(),
            }),
            ..Default::default()
        };
        let meta = build_claude_code_options_meta(Some(&options), false).unwrap();
        assert_eq!(meta["systemPrompt"]["append"], "and be nice");
    }

    #[test]
    fn isolate_user_settings_adds_setting_sources_even_without_options() {
        let meta = build_claude_code_options_meta(None, true).unwrap();
        let sources = meta["claudeCode"]["options"]["settingSources"]
            .as_array()
            .unwrap();
        assert_eq!(sources, &vec![Value::from("project"), Value::from("local")]);
    }

    #[test]
    fn include_user_settings_env_prepends_user_source() {
        unsafe {
            std::env::set_var("ACPX_CLAUDE_INCLUDE_USER_SETTINGS", "1");
        }
        let sources = resolve_claude_code_setting_sources();
        unsafe {
            std::env::remove_var("ACPX_CLAUDE_INCLUDE_USER_SETTINGS");
        }
        assert_eq!(sources, vec!["user", "project", "local"]);
    }
}
