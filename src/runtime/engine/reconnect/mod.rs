//! Reconnect state machine. Ports
//! `others/acpx/src/runtime/engine/reconnect.ts`'s `connectAndLoadSession`
//! orchestration (`loadOrCreateRuntimeSession` and its helpers) — the most
//! state-machine-heavy file in the whole scoped port (Risk Assessment #1).
//!
//! ## State machine (Step 6 — designed before transliterating)
//!
//! Rather than acpx's flat sequence of nested `if`/`try`/`catch`, every
//! state acpx implicitly tracks is named here so a reviewer can check each
//! transition is handled, instead of trusting a 400-line imperative
//! function reads them all correctly:
//!
//! ```text
//! entry: caller already confirmed there is no live in-memory
//!        ConnectedSession for this record (see connected_session.rs docs —
//!        unlike acpx's daemon, this crate's host is one continuously
//!        running process, so "already connected" is checked one layer up,
//!        not inside this module).
//!
//! [ChooseAcquisitionPath] -- based on agent_capabilities + same_session_only
//!   -> Resume            (agent advertises session_capabilities.resume)
//!   -> Load              (agent advertises load_session, no resume)
//!   -> RequireSameSession (neither, and same_session_only)
//!   -> CreateFresh        (neither, same_session_only == false)
//!
//! [Resume] / [Load] -- send session/resume or session/load
//!   -> Acquired { resumed: true }                    on success
//!   -> propagate AcpError::Timeout                    hard failure (never falls back)
//!   -> Err(SessionResumeRequired)                     same_session_only == true
//!   -> [CreateFresh] with load_error recorded          fallback-eligible RPC error
//!                                                       (resource-not-found / unsupported
//!                                                       method / query-closed-before-response
//!                                                       or generic internal error on a session
//!                                                       with no agent replies yet)
//!   -> propagate normalized AcpError                   anything else (would silently lose
//!                                                       real conversation history)
//!
//! [RequireSameSession] -> Err(SessionResumeRequired)
//!
//! [CreateFresh] -- session/new
//!   -> Acquired { resumed: false, created_fresh: true }  (no further fallback if this fails)
//!
//! [Acquired { created_fresh: true }] -- only if the record has any desired
//! mode/model/config-option state to replay
//!   -> replay mode, then model, then config options (in that order)
//!   -> Done                                            replay succeeded (or nothing to replay)
//!   -> Err(SessionModeReplay | SessionModelReplay | SessionConfigOptionReplay)
//!      + record rolled back to its pre-attempt acp_session_id/agent_session_id/acpx
//!
//! [Acquired { created_fresh: false }] -> Done            (resumed/loaded session already has
//!                                                          these settings; skip replay)
//! ```

pub mod liveness;
pub mod replay;

use std::path::PathBuf;
use std::time::Duration;

use agent_client_protocol::Error as AcpRpcError;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, LoadSessionRequest, McpServer, ResumeSessionRequest, SessionId,
};

use crate::agent_command::model_support::{SessionModelState, model_state_from_session_response};
use crate::agent_command::{
    build_claude_acp_session_create_timeout_message, is_claude_acp_command,
    resolve_claude_acp_session_create_timeout_ms, split_command_line,
};
use crate::agent_session_id::extract_agent_session_id;
use crate::client::AcpClient;
use crate::control::with_timeout;
use crate::error::{AcpError, Result};
use crate::error_normalization::{
    is_acp_query_closed_before_response_error, normalize_agent_error,
};
use crate::error_shapes::is_acp_resource_not_found_error;
use crate::session::config_options::apply_config_options_to_record;
use crate::session::conversation_model::SessionConversation;
use crate::session::mode_preference::{get_desired_config_options, get_desired_model_id};
use crate::session::model_state::{advertised_model_state, apply_reconnected_model_state};
use crate::session::record::SessionRecord;
use crate::types::SessionResumePolicy;

use super::session_options::{build_claude_code_options_meta, session_options_from_record};
use replay::{has_preferences_to_replay, replay_fresh_session_preferences};

/// Result of [`connect_and_load_session`].
pub struct ConnectAndLoadSessionResult {
    pub session_id: SessionId,
    pub agent_session_id: Option<String>,
    pub resumed: bool,
    pub load_error: Option<String>,
}

/// Gap 6/15/23: the response-derived model-state inputs
/// [`crate::session::model_state::apply_reconnected_model_state`] needs,
/// computed once per acquisition RPC response (shared by
/// [`create_fresh_session`] and [`acquire_via_rpc`]) so gap 15's
/// `legacyModelMetadataPresent` flag and gap 23's `model_state_from_session_response`
/// call aren't duplicated.
struct AcquiredModelState {
    config_options_present: bool,
    legacy_model_metadata_present: bool,
    session_models: Option<SessionModelState>,
}

fn model_state_from_response(
    config_options: Option<&[agent_client_protocol::schema::v1::SessionConfigOption]>,
    meta: Option<&agent_client_protocol::schema::v1::Meta>,
) -> AcquiredModelState {
    let legacy_meta_value = meta.map(|m| serde_json::Value::Object(m.clone()));
    let legacy_model_metadata_present = legacy_meta_value
        .as_ref()
        .is_some_and(|m| m.get("models").is_some());
    let session_models = model_state_from_session_response(
        config_options.unwrap_or(&[]),
        legacy_meta_value.as_ref(),
    );
    AcquiredModelState {
        config_options_present: config_options.is_some(),
        legacy_model_metadata_present,
        session_models,
    }
}

const UNSUPPORTED_SESSION_LOAD_CODES: [i32; 2] = [-32601, -32602];

fn session_resume_required(record: &SessionRecord, reason: &str) -> AcpError {
    AcpError::SessionResumeRequired(format!(
        "Persistent ACP session {} could not be resumed: {reason}",
        record.acp_session_id
    ))
}

/// Ports `shouldFallbackToNewSession`'s RPC-error-classification half (the
/// hard-failure and same-session-only paths are handled by
/// [`acquire_via_rpc`] before this is consulted).
fn should_fallback_to_new_session(error: &AcpRpcError, conversation: &SessionConversation) -> bool {
    let code: i32 = error.code.into();
    if is_acp_resource_not_found_error(error) || UNSUPPORTED_SESSION_LOAD_CODES.contains(&code) {
        return true;
    }
    if crate::runtime::engine::lifecycle::session_has_agent_messages(conversation) {
        return false;
    }
    is_acp_query_closed_before_response_error(error) || code == -32603
}

enum AcquisitionPath {
    Resume,
    Load,
    RequireSameSession,
    CreateFresh,
}

/// Ports the branching at the top of `loadOrCreateRuntimeSession`.
fn choose_acquisition_path(
    capabilities: &AgentCapabilities,
    same_session_only: bool,
) -> AcquisitionPath {
    if capabilities.session_capabilities.resume.is_some() {
        AcquisitionPath::Resume
    } else if capabilities.load_session {
        AcquisitionPath::Load
    } else if same_session_only {
        AcquisitionPath::RequireSameSession
    } else {
        AcquisitionPath::CreateFresh
    }
}

/// Outcome of acquiring a live backend session id, before any preference
/// replay. Ports `RuntimeSessionLoadState`.
struct Acquired {
    session_id: SessionId,
    agent_session_id: Option<String>,
    resumed: bool,
    created_fresh: bool,
    load_error: Option<String>,
    /// Gap 15/23: the response-derived model-state inputs
    /// `connect_and_load_session`'s tail feeds into `apply_reconnected_model_state`.
    model_state: AcquiredModelState,
}

/// Ports the shared body of `resumeRuntimeSession`/`loadRuntimeSession`:
/// sends the raw request (bypassing [`AcpClient`]'s normalized wrappers so
/// this function can classify the *raw* JSON-RPC code, matching
/// `extractAcpError`'s use in acpx), then either succeeds, falls back to
/// [`create_fresh_session`], or propagates.
async fn acquire_via_rpc(
    client: &AcpClient,
    record: &mut SessionRecord,
    conversation: &SessionConversation,
    mcp_servers: &[McpServer],
    same_session_only: bool,
    timeout: Option<Duration>,
    use_resume: bool,
) -> Result<Acquired> {
    let session_id = SessionId::new(record.acp_session_id.clone());
    let cwd = PathBuf::from(&record.cwd);

    // `ResumeSessionResponse`/`LoadSessionResponse` share the same
    // `modes`/`config_options`/`meta` shape but are distinct types, so each
    // branch normalizes its response into a common `(meta, config_options)`
    // pair immediately rather than trying to unify the two response types
    // at the `if`/`else` boundary.
    let raw_result: std::result::Result<
        (
            Option<agent_client_protocol::schema::v1::Meta>,
            Option<Vec<agent_client_protocol::schema::v1::SessionConfigOption>>,
        ),
        AcpRpcError,
    > = if use_resume {
        let request =
            ResumeSessionRequest::new(session_id.clone(), cwd).mcp_servers(mcp_servers.to_vec());
        let timed = with_timeout(
            client.connection().send_request(request).block_task(),
            timeout,
        )
        .await?;
        timed.map(|response| (response.meta, response.config_options))
    } else {
        let request =
            LoadSessionRequest::new(session_id.clone(), cwd).mcp_servers(mcp_servers.to_vec());
        let timed = with_timeout(
            client.connection().send_request(request).block_task(),
            timeout,
        )
        .await?;
        timed.map(|response| (response.meta, response.config_options))
    };

    match raw_result {
        Ok((meta, config_options)) => {
            let agent_session_id = extract_agent_session_id(meta.as_ref());
            let model_state = model_state_from_response(config_options.as_deref(), meta.as_ref());
            apply_config_options_to_record(record, config_options);
            Ok(Acquired {
                session_id,
                agent_session_id,
                resumed: true,
                created_fresh: false,
                load_error: None,
                model_state,
            })
        }
        Err(rpc_error) => {
            if same_session_only {
                return Err(session_resume_required(
                    record,
                    &format!("{} (ACP {})", rpc_error.message, i32::from(rpc_error.code)),
                ));
            }
            if should_fallback_to_new_session(&rpc_error, conversation) {
                let load_error =
                    format!("{} (ACP {})", rpc_error.message, i32::from(rpc_error.code));
                return create_fresh_session(
                    client,
                    record,
                    mcp_servers,
                    timeout,
                    Some(load_error),
                )
                .await;
            }
            Err(normalize_agent_error(
                rpc_error,
                record.acp_session_id.clone(),
            ))
        }
    }
}

/// Ports `createFreshRuntimeSession`.
async fn create_fresh_session(
    client: &AcpClient,
    record: &mut SessionRecord,
    mcp_servers: &[McpServer],
    timeout: Option<Duration>,
    load_error: Option<String>,
) -> Result<Acquired> {
    let cwd = PathBuf::from(&record.cwd);
    // Gap 4: Claude Code's ACP adapter can hang indefinitely on `session/new`.
    // For a Claude command, substitute the caller's generic timeout with the
    // Claude-specific default (env-overridable, 60s) and map an expiry to the
    // dedicated `ClaudeAcpSessionCreateTimeout` diagnostic instead of a
    // generic timeout, matching acpx.
    let is_claude = split_command_line(&record.agent_command)
        .map(|parts| is_claude_acp_command(&parts.command, &parts.args))
        .unwrap_or(false);
    let effective_timeout = if is_claude {
        Some(resolve_claude_acp_session_create_timeout_ms())
    } else {
        timeout
    };
    // Gap 10: attach `_meta.claudeCode.options` (built from the record's
    // persisted `SessionAgentOptions`) unconditionally — non-Claude agents
    // ignore unrecognized `_meta` keys per ACP's extensibility convention.
    // `isolate_user_settings` (acpx's `isolateUserSettings`) mirrors
    // acpx's own `createSession` call site: gated on this being a Claude
    // ACP command specifically.
    let session_agent_options = session_options_from_record(record);
    let meta = build_claude_code_options_meta(session_agent_options.as_ref(), is_claude);
    let response = match with_timeout(
        client.session_new_with_meta(cwd, mcp_servers.to_vec(), meta),
        effective_timeout,
    )
    .await
    {
        Ok(result) => result?,
        Err(AcpError::Timeout(_)) if is_claude => {
            return Err(AcpError::ClaudeAcpSessionCreateTimeout(
                build_claude_acp_session_create_timeout_message(),
            ));
        }
        Err(other) => return Err(other),
    };
    let agent_session_id = extract_agent_session_id(response.meta.as_ref());
    let model_state =
        model_state_from_response(response.config_options.as_deref(), response.meta.as_ref());
    apply_config_options_to_record(record, response.config_options.clone());
    Ok(Acquired {
        session_id: response.session_id,
        agent_session_id,
        resumed: false,
        created_fresh: true,
        load_error,
        model_state,
    })
}

async fn acquire_session(
    client: &AcpClient,
    record: &mut SessionRecord,
    conversation: &SessionConversation,
    mcp_servers: &[McpServer],
    same_session_only: bool,
    timeout: Option<Duration>,
) -> Result<Acquired> {
    // No backend session id has ever been assigned yet (a genuinely new
    // record, e.g. the first `ensure_session` call for this key) — there is
    // nothing to resume or load, so always create fresh regardless of
    // capabilities or `same_session_only`. Not present in acpx's own
    // `loadOrCreateRuntimeSession` because acpx's manager never calls this
    // path for a record it just minted itself (it inlines fresh-session
    // creation there instead); this port funnels both cases through the
    // same state machine, so the empty-id guard has to live here.
    if record.acp_session_id.trim().is_empty() {
        return create_fresh_session(client, record, mcp_servers, timeout, None).await;
    }

    let capabilities = client.init_response().agent_capabilities.clone();
    match choose_acquisition_path(&capabilities, same_session_only) {
        AcquisitionPath::Resume => {
            acquire_via_rpc(
                client,
                record,
                conversation,
                mcp_servers,
                same_session_only,
                timeout,
                true,
            )
            .await
        }
        AcquisitionPath::Load => {
            acquire_via_rpc(
                client,
                record,
                conversation,
                mcp_servers,
                same_session_only,
                timeout,
                false,
            )
            .await
        }
        AcquisitionPath::RequireSameSession => Err(session_resume_required(
            record,
            "agent does not support session/resume or session/load",
        )),
        AcquisitionPath::CreateFresh => {
            create_fresh_session(client, record, mcp_servers, timeout, None).await
        }
    }
}

/// Ports `connectAndLoadSession`. `conversation` is the connected session's
/// in-memory conversation model (used only to answer
/// `sessionHasAgentMessages` for the fallback-safety check — the caller
/// still owns writing turn content into it).
pub async fn connect_and_load_session(
    client: &AcpClient,
    record: &mut SessionRecord,
    conversation: &SessionConversation,
    mcp_servers: &[McpServer],
    resume_policy: SessionResumePolicy,
    timeout: Option<Duration>,
) -> Result<ConnectAndLoadSessionResult> {
    let same_session_only = matches!(resume_policy, SessionResumePolicy::SameSessionOnly)
        || record.imported_from.is_some();

    let original_session_id = record.acp_session_id.clone();
    let original_agent_session_id = record.agent_session_id.clone();
    let original_acpx = record.acpx.clone();
    let agent_command = record.agent_command.clone();

    let acquired = acquire_session(
        client,
        record,
        conversation,
        mcp_servers,
        same_session_only,
        timeout,
    )
    .await?;

    record.acp_session_id = acquired.session_id.0.to_string();
    if let Some(agent_session_id) = &acquired.agent_session_id {
        crate::runtime::engine::lifecycle::reconcile_agent_session_id(
            record,
            Some(agent_session_id),
        );
    }

    // Gap 15: whether replay would touch `config_options` at all (model or
    // config-option replay — mode replay alone never adds config options),
    // computed from the record's PRE-replay "desired" state. Combined with
    // whether replay actually ran below, this approximates acpx's
    // `resolveConfigOptionsPresenceAfterReplay` (`initiallyPresent ||
    // configReplay.replayed || (modelReplay.replayed &&
    // modelReplay.configOptionsPresent)`) without needing `replay.rs` (out
    // of this phase's file scope) to return the finer per-step replay
    // results.
    let replay_would_touch_config_options = acquired.created_fresh
        && (get_desired_model_id(record.acpx.as_ref()).is_some()
            || !get_desired_config_options(record.acpx.as_ref()).is_empty());
    let mut replay_ran = false;

    if acquired.created_fresh && has_preferences_to_replay(record) {
        replay_ran = true;
        let replay_result = replay_fresh_session_preferences(
            client,
            acquired.session_id.clone(),
            record,
            &agent_command,
        )
        .await;
        if let Err(err) = replay_result {
            record.acp_session_id = original_session_id;
            record.agent_session_id = original_agent_session_id;
            record.acpx = original_acpx;
            return Err(err);
        }
    }

    // Gap 15/23: reconcile the record's persisted model/config-option state
    // against what this connection actually observed, regardless of which
    // acquisition path ran (Requirement 5). If replay ran and touched model
    // or config-option state, `record.acpx` already reflects the
    // post-replay config options (via `apply_config_options_to_record`
    // inside `replay.rs`) — re-derive `session_models` from that current
    // state rather than the pre-replay acquisition response so the
    // reconciliation sees the same values the replay just applied.
    let config_options_present = acquired.model_state.config_options_present
        || (replay_ran && replay_would_touch_config_options);
    let session_models = if replay_ran && replay_would_touch_config_options {
        advertised_model_state(record.acpx.as_ref())
    } else {
        acquired.model_state.session_models.clone()
    };
    apply_reconnected_model_state(
        record,
        session_models.as_ref(),
        config_options_present,
        acquired.model_state.legacy_model_metadata_present,
        acquired.created_fresh,
    );

    Ok(ConnectAndLoadSessionResult {
        session_id: acquired.session_id,
        agent_session_id: record.agent_session_id.clone(),
        resumed: acquired.resumed,
        load_error: acquired.load_error,
    })
}

#[cfg(test)]
mod tests {
    //! Gap 5: direct unit coverage for the acquisition-path selection and
    //! fallback-classification logic — the reconnect state machine was
    //! previously the highest-risk file in the crate with zero unit tests.
    use super::*;
    use crate::session::conversation_model::conversation::create_session_conversation;

    fn capabilities(resume: bool, load: bool) -> AgentCapabilities {
        let mut caps = AgentCapabilities::new();
        caps.load_session = load;
        if resume {
            caps.session_capabilities.resume = Some(Default::default());
        }
        caps
    }

    #[test]
    fn resume_capability_wins_over_load_and_same_session() {
        assert!(matches!(
            choose_acquisition_path(&capabilities(true, true), true),
            AcquisitionPath::Resume
        ));
        assert!(matches!(
            choose_acquisition_path(&capabilities(true, false), false),
            AcquisitionPath::Resume
        ));
    }

    #[test]
    fn load_chosen_when_only_load_advertised() {
        assert!(matches!(
            choose_acquisition_path(&capabilities(false, true), true),
            AcquisitionPath::Load
        ));
    }

    #[test]
    fn require_same_session_when_neither_and_same_session_only() {
        assert!(matches!(
            choose_acquisition_path(&capabilities(false, false), true),
            AcquisitionPath::RequireSameSession
        ));
    }

    #[test]
    fn create_fresh_when_neither_and_not_same_session_only() {
        assert!(matches!(
            choose_acquisition_path(&capabilities(false, false), false),
            AcquisitionPath::CreateFresh
        ));
    }

    #[test]
    fn fallback_true_for_unsupported_method_codes() {
        let conversation = create_session_conversation(None);
        for code in UNSUPPORTED_SESSION_LOAD_CODES {
            let err = AcpRpcError::new(code, "method not found");
            assert!(
                should_fallback_to_new_session(&err, &conversation),
                "code {code} should fall back to a fresh session"
            );
        }
    }

    #[test]
    fn fallback_true_for_internal_error_when_no_agent_messages() {
        let conversation = create_session_conversation(None);
        let err = AcpRpcError::new(-32603, "internal error");
        assert!(should_fallback_to_new_session(&err, &conversation));
    }

    #[test]
    fn fallback_false_for_unclassified_code_with_no_agent_messages() {
        let conversation = create_session_conversation(None);
        // -32000 is neither resource-not-found, unsupported, query-closed,
        // nor the -32603 internal-error code -> must NOT silently fall back.
        let err = AcpRpcError::new(-32000, "server error");
        assert!(!should_fallback_to_new_session(&err, &conversation));
    }
}
