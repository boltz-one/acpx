//! Pure helper functions for `manager.rs` — record construction, status/
//! usage projection, and the already-failed-turn builder. Split out purely
//! to stay under this crate's per-file line convention; logically part of
//! `manager.rs` (ports the same free-function helpers from acpx's
//! `manager.ts`: `createInitialRecord`, `statusSummary`, `buildUsageField`,
//! `tokenUsageToBreakdown`, `legacyTerminalEventFromTurnResult`'s sibling
//! error-turn construction, etc).

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::error::AcpError;
use crate::session::acpx_state::SessionAcpxState;
use crate::session::conversation_model::conversation::{SessionConversation, SessionTokenUsage};
use crate::session::conversation_model::iso_now;
use crate::session::event_log::SessionEventLog;
use crate::session::record::SessionRecord;
use crate::session::schema::SessionSchemaVersion;
use crate::types::SessionResumePolicy;

use crate::runtime::public::contract::{
    AcpRuntimeSessionMode, AcpRuntimeSessionModels, AcpRuntimeSessionUsage, AcpRuntimeStatus,
    AcpRuntimeTurn, AcpRuntimeTurnResult, AcpRuntimeTurnResultError,
};
use crate::runtime::public::errors::{AcpRuntimeError, AcpRuntimeErrorCode};
use crate::runtime::public::events::{
    AcpRuntimeAvailableCommand, AcpRuntimeEvent, AcpRuntimeUsageBreakdown, AcpRuntimeUsageCost,
};

pub(super) fn wrap_err(code: AcpRuntimeErrorCode, context: &str, err: AcpError) -> AcpRuntimeError {
    AcpRuntimeError::with_cause(code, format!("{context}: {err}"), err)
}

pub(super) fn record_id_for(session_key: &str, mode: AcpRuntimeSessionMode) -> String {
    match mode {
        AcpRuntimeSessionMode::Persistent => session_key.to_string(),
        AcpRuntimeSessionMode::Oneshot => {
            format!("{session_key}:oneshot:{}", uuid::Uuid::new_v4())
        }
    }
}

pub(super) fn resume_policy_for_mode(mode: AcpRuntimeSessionMode) -> SessionResumePolicy {
    match mode {
        AcpRuntimeSessionMode::Persistent => SessionResumePolicy::SameSessionOnly,
        AcpRuntimeSessionMode::Oneshot => SessionResumePolicy::AllowNew,
    }
}

pub(super) fn create_initial_record(
    record_id: &str,
    agent_command: &str,
    cwd: &str,
) -> SessionRecord {
    let now = iso_now();
    SessionRecord {
        schema: SessionSchemaVersion::default(),
        acpx_record_id: record_id.to_string(),
        acp_session_id: String::new(),
        agent_session_id: None,
        agent_command: agent_command.to_string(),
        cwd: cwd.to_string(),
        name: None,
        created_at: now.clone(),
        last_used_at: now.clone(),
        last_seq: 0,
        last_request_id: None,
        // The event-log NDJSON segment scaffolding is a file-store-specific
        // concern (see `session::event_log`'s `default_session_event_log`,
        // which needs an `AcpFileSessionStoreOptions`); this manager only
        // depends on the abstract `AcpSessionStore` trait, so fresh records
        // start with the path-less default rather than assuming a
        // file-backed store.
        event_log: SessionEventLog::default(),
        closed: false,
        closed_at: None,
        pid: None,
        agent_started_at: None,
        last_prompt_at: None,
        last_agent_exit_code: None,
        last_agent_exit_signal: None,
        last_agent_exit_at: None,
        last_agent_disconnect_reason: None,
        protocol_version: None,
        agent_capabilities: None,
        title: None,
        messages: Vec::new(),
        updated_at: now,
        cumulative_token_usage: Default::default(),
        cumulative_cost: None,
        request_token_usage: IndexMap::new(),
        acpx: Some(SessionAcpxState::default()),
        imported_from: None,
        extra: Default::default(),
    }
}

pub(super) fn conversation_from_record(record: &SessionRecord) -> SessionConversation {
    SessionConversation {
        title: record.title.clone(),
        messages: record.messages.clone(),
        updated_at: record.updated_at.clone(),
        cumulative_token_usage: record.cumulative_token_usage,
        cumulative_cost: record.cumulative_cost.clone(),
        request_token_usage: record.request_token_usage.clone(),
    }
}

fn status_summary(record: &SessionRecord) -> String {
    let mut parts = vec![
        format!("session={}", record.acpx_record_id),
        format!("backendSessionId={}", record.acp_session_id),
    ];
    if let Some(id) = &record.agent_session_id {
        parts.push(format!("agentSessionId={id}"));
    }
    if let Some(pid) = record.pid {
        parts.push(format!("pid={pid}"));
    }
    parts.push(if record.closed { "closed" } else { "open" }.to_string());
    parts.join(" ")
}

fn token_usage_to_breakdown(usage: SessionTokenUsage) -> Option<AcpRuntimeUsageBreakdown> {
    let breakdown = AcpRuntimeUsageBreakdown {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_read_tokens: usage.cache_read_input_tokens,
        cached_write_tokens: usage.cache_creation_input_tokens,
        thought_tokens: usage.thought_tokens,
        total_tokens: usage.total_tokens,
    };
    (!breakdown.is_empty()).then_some(breakdown)
}

fn build_usage_field(record: &SessionRecord) -> Option<AcpRuntimeSessionUsage> {
    let cumulative = token_usage_to_breakdown(record.cumulative_token_usage);
    let per_request: HashMap<String, AcpRuntimeUsageBreakdown> = record
        .request_token_usage
        .iter()
        .filter_map(|(id, usage)| token_usage_to_breakdown(*usage).map(|b| (id.clone(), b)))
        .collect();
    let cost = record
        .cumulative_cost
        .as_ref()
        .map(|cost| AcpRuntimeUsageCost {
            amount: cost.amount,
            currency: cost.currency.clone(),
        });
    if cumulative.is_none() && per_request.is_empty() && cost.is_none() {
        return None;
    }
    Some(AcpRuntimeSessionUsage {
        cumulative,
        cost,
        per_request,
    })
}

pub(super) fn runtime_status_from_record(record: &SessionRecord) -> AcpRuntimeStatus {
    let acpx = record.acpx.as_ref();
    let models = acpx
        .and_then(|a| a.available_models.as_ref())
        .map(|models| AcpRuntimeSessionModels {
            current_model_id: acpx.and_then(|a| a.current_model_id.clone()),
            available_model_ids: models.clone(),
        });
    let available_commands = acpx
        .and_then(|a| a.available_commands.as_ref())
        .map(|commands| {
            commands
                .iter()
                .map(|command| AcpRuntimeAvailableCommand {
                    name: command.name.clone(),
                    description: command.description.clone(),
                    has_input: command.has_input,
                })
                .collect()
        });
    AcpRuntimeStatus {
        summary: Some(status_summary(record)),
        acpx_record_id: Some(record.acpx_record_id.clone()),
        backend_session_id: Some(record.acp_session_id.clone()),
        agent_session_id: record.agent_session_id.clone(),
        models,
        usage: build_usage_field(record),
        available_commands,
    }
}

/// Builds an already-failed [`AcpRuntimeTurn`] for `start_turn` callers that
/// hit an error before any RPC work could begin (no connected session, or
/// unsupported attachment media type). Its `cancel()` is a documented no-op
/// (nothing is listening on the paired receiver — there is no background
/// task for a turn that never started).
pub(super) fn failed_turn(request_id: String, err: AcpRuntimeError) -> AcpRuntimeTurn {
    use futures::stream::BoxStream;

    let events: BoxStream<'static, AcpRuntimeEvent> = Box::pin(futures::stream::empty());
    let result = Box::pin(async move {
        AcpRuntimeTurnResult::Failed {
            error: AcpRuntimeTurnResultError {
                message: err.message,
                code: Some(err.code.as_str().to_string()),
                detail_code: None,
                retryable: Some(false),
            },
        }
    });
    let (cancel_tx, _cancel_rx) = futures::channel::oneshot::channel();
    AcpRuntimeTurn::new(request_id, events, result, cancel_tx)
}
