//! Verifying an imported archive's agent identity against the caller's
//! expectation.
//!
//! Ports `assertExpectedAgentCommand`/`agentCommandMatchesExpected` from
//! `others/acpx/src/session/import.ts`. acpx's `commandLooksLikeBuiltInAgent`
//! regex-matches specific npm package names
//! (`@agentclientprotocol/codex-acp`, etc.) against the archived command
//! string; this port uses a simpler match against this crate's own
//! `crate::agent_command::normalize_agent_name` instead, since acpx's
//! npm-package-name heuristics don't apply to this port's own command
//! resolution conventions.

use crate::error::Result;
use crate::session::export::{ExportedSession, normalize_agent_identity};
use crate::session::record::SessionRecord;

use super::{ImportSessionOptions, import_error};

fn command_looks_like_built_in_agent(command: &str, agent_name: &str) -> bool {
    crate::agent_command::normalize_agent_name(command)
        == crate::agent_command::normalize_agent_name(agent_name)
}

fn agent_command_matches_expected(
    archived_command: &str,
    expected_agent_command: &str,
    expected_agent_name: Option<&str>,
) -> bool {
    if archived_command == expected_agent_command {
        return true;
    }
    expected_agent_name
        .map(|name| command_looks_like_built_in_agent(archived_command, name))
        .unwrap_or(false)
}

/// Ports `archiveAgentNameMatches`. Short-circuits to `true` when both the
/// archive's and the source record's commands already literally equal the
/// expected command (an exact-command match trumps a name mismatch);
/// otherwise falls back to comparing the normalized agent names, treating
/// either side being absent as a permissive pass. This is the previously
/// missing 3rd AND-condition: without it, an archive whose command string
/// happens to match the expected command but whose declared agent name has
/// been spoofed/differs would be silently accepted.
fn archive_agent_name_matches(
    archive_agent_name: Option<&str>,
    expected_agent_name: Option<&str>,
    archive_command: &str,
    state_command: &str,
    expected_agent_command: &str,
) -> bool {
    if archive_command == expected_agent_command && state_command == expected_agent_command {
        return true;
    }
    archive_agent_name.is_none()
        || expected_agent_name.is_none()
        || archive_agent_name == expected_agent_name
}

/// Ports `assertExpectedAgentCommand`.
pub(super) fn assert_expected_agent_command(
    archive: &ExportedSession,
    source_record: &mut SessionRecord,
    options: &ImportSessionOptions,
) -> Result<()> {
    let Some(expected_agent_command) = &options.expected_agent_command else {
        return Ok(());
    };
    let expected_agent_name = normalize_agent_identity(options.expected_agent_name.as_deref());
    let archive_agent_name = normalize_agent_identity(archive.session.agent_name.as_deref());
    let archive_command_matches = agent_command_matches_expected(
        &archive.session.agent,
        expected_agent_command,
        expected_agent_name.as_deref(),
    );
    let state_command_matches = agent_command_matches_expected(
        &source_record.agent_command,
        expected_agent_command,
        expected_agent_name.as_deref(),
    );
    let agent_name_matches = archive_agent_name_matches(
        archive_agent_name.as_deref(),
        expected_agent_name.as_deref(),
        &archive.session.agent,
        &source_record.agent_command,
        expected_agent_command,
    );

    if archive_command_matches && state_command_matches && agent_name_matches {
        source_record.agent_command = expected_agent_command.clone();
        return Ok(());
    }
    Err(import_error(
        "Session export archive agent does not match the requested agent",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::export::ExportedSessionInfo;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    fn exported_session(agent: &str, agent_name: Option<&str>) -> ExportedSession {
        ExportedSession {
            format_version: 1,
            exported_at: "2026-01-01T00:00:00Z".to_string(),
            exported_by: "boltz-acpx".to_string(),
            session: ExportedSessionInfo {
                record_id: "record-1".to_string(),
                name: None,
                agent: agent.to_string(),
                agent_name: agent_name.map(str::to_string),
                cwd_relative: "project".to_string(),
                cwd_original: "project".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
                state: serde_json::json!({}),
            },
            history: Vec::new(),
        }
    }

    // --- Unit tests for `archive_agent_name_matches` (the 5 acpx-spec
    // scenarios from the plan's Step 5). ---

    #[test]
    fn short_circuits_true_when_both_commands_literally_equal_expected_even_if_names_differ() {
        assert!(archive_agent_name_matches(
            Some("claude"),
            Some("gpt"),
            "cmd-x",
            "cmd-x",
            "cmd-x",
        ));
    }

    #[test]
    fn falls_back_true_when_archive_name_is_none() {
        assert!(archive_agent_name_matches(
            None,
            Some("gpt"),
            "cmd-a",
            "cmd-b",
            "cmd-x",
        ));
    }

    #[test]
    fn falls_back_true_when_expected_name_is_none() {
        assert!(archive_agent_name_matches(
            Some("claude"),
            None,
            "cmd-a",
            "cmd-b",
            "cmd-x",
        ));
    }

    #[test]
    fn falls_back_true_when_names_present_and_equal() {
        assert!(archive_agent_name_matches(
            Some("claude"),
            Some("claude"),
            "cmd-a",
            "cmd-b",
            "cmd-x",
        ));
    }

    #[test]
    fn falls_back_false_when_names_present_and_differ_and_no_short_circuit() {
        // Security-relevant case: commands don't both literally equal the
        // expected command, so the short-circuit doesn't apply, and the
        // agent names genuinely differ — must be rejected.
        assert!(!archive_agent_name_matches(
            Some("claude"),
            Some("gpt"),
            "cmd-a",
            "cmd-b",
            "cmd-x",
        ));
    }

    // --- Integration test on `assert_expected_agent_command`: proves the
    // previously-missing check now actually rejects an archive whose
    // command strings match the expected agent (via the
    // `command_looks_like_built_in_agent` heuristic, not literal equality)
    // but whose declared agent name has been spoofed/differs. ---

    #[test]
    fn rejects_import_when_commands_match_via_heuristic_but_agent_names_differ() {
        let archive = exported_session("Claude", Some("codex"));
        let mut source_record = sample_session_record();
        source_record.agent_command = "CLAUDE".to_string();
        let options = ImportSessionOptions {
            expected_agent_name: Some("claude".to_string()),
            expected_agent_command: Some("claude-code-cli".to_string()),
            ..Default::default()
        };

        let err = assert_expected_agent_command(&archive, &mut source_record, &options)
            .expect_err("mismatched agent name must be rejected");
        assert!(matches!(err, crate::error::AcpError::SessionResolution(_)));
    }

    #[test]
    fn accepts_import_when_commands_literally_equal_expected_despite_name_mismatch() {
        let archive = exported_session("claude-code-cli", Some("codex"));
        let mut source_record = sample_session_record();
        source_record.agent_command = "claude-code-cli".to_string();
        let options = ImportSessionOptions {
            expected_agent_name: Some("claude".to_string()),
            expected_agent_command: Some("claude-code-cli".to_string()),
            ..Default::default()
        };

        assert_expected_agent_command(&archive, &mut source_record, &options)
            .expect("literal command match short-circuits the name check");
        assert_eq!(source_record.agent_command, "claude-code-cli");
    }
}
