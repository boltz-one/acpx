//! `spawn_connected_session`: builds the filesystem/terminal/permission
//! handlers for one session, spawns its `AcpClient`, runs the reconnect
//! state machine, and persists the result. Split out of `manager.rs` purely
//! for the per-file line convention — takes `&AcpRuntimeOptions` rather than
//! `&AcpRuntime` since that's all it actually needs.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{SessionConfigId, SessionConfigValueId};

use crate::agent_command::model_request::{
    assert_requested_model_supported, resolve_requested_model_id,
};
use crate::agent_command::resolve_agent_program;
use crate::auth_env::build_agent_environment;
use crate::client::handlers::{ClientRequestHandlers, PermissionRequestWiring};
use crate::client::{AcpClient, SpawnAgentOptions};
use crate::filesystem::{ClientOperation, FilesystemHandlers};
use crate::session::conversation_model::iso_now;
use crate::session::model_state::advertised_model_state;
use crate::session::record::SessionRecord;
use crate::terminal::{TerminalManager, TerminalManagerOptions};
use crate::types::SessionResumePolicy;

use super::connected_session::{ConnectedSession, drain_replay_notifications};
use super::manager_support::{conversation_from_record, wrap_err};
use super::reconnect::connect_and_load_session;
use super::session_options::session_options_from_record;

use crate::runtime::public::contract::AcpRuntimeOptions;
use crate::runtime::public::errors::{AcpRuntimeError, AcpRuntimeErrorCode};

/// Gap 12: after a session connects, if the caller's persisted
/// `session_options.model` names a model, apply it to the live agent
/// connection (mirroring acpx's `applyRequestedModelIfAdvertised`, called
/// unconditionally — including for resumed sessions, per the phase's
/// confirmed Locked-in Decision) and persist the resulting current-model-id
/// onto the record. Failures are logged, not propagated — a session whose
/// requested model can't be honored should still come up (matching acpx's
/// own warn-and-continue behavior for this call site), just without the
/// model actually switched.
async fn apply_requested_model_if_advertised(client: &AcpClient, record: &mut SessionRecord) {
    let Some(requested_model) = session_options_from_record(record).and_then(|o| o.model) else {
        return;
    };
    if requested_model.trim().is_empty() {
        return;
    }

    let agent_command = record.agent_command.clone();
    let advertised = advertised_model_state(record.acpx.as_ref());
    let warning = match assert_requested_model_supported(
        &requested_model,
        advertised.as_ref(),
        Some(&agent_command),
        false,
    ) {
        Ok(warning) => warning,
        Err(err) => {
            log::warn!(
                "[acp] cannot apply requested model \"{requested_model}\": {}",
                err.message
            );
            return;
        }
    };
    if let Some(warning) = warning {
        log::info!("[acp] {warning}");
    }

    let resolved_model_id =
        resolve_requested_model_id(&requested_model, advertised.as_ref(), Some(&agent_command));
    let config_id = advertised
        .as_ref()
        .and_then(|models| models.config_id.clone())
        .unwrap_or_else(|| "model".to_string());
    let session_id =
        agent_client_protocol::schema::v1::SessionId::new(record.acp_session_id.clone());

    match client
        .set_session_config_option(
            session_id,
            SessionConfigId::new(config_id),
            SessionConfigValueId::new(resolved_model_id.clone()),
        )
        .await
    {
        Ok(response) => {
            let current_model_id =
                crate::session::model_application::current_model_id_from_set_model_response(
                    Some(response.config_options.as_slice()),
                    Some(&resolved_model_id),
                );
            crate::session::config_options::apply_config_options_to_record(
                record,
                Some(response.config_options),
            );
            if let (Some(current_model_id), Some(acpx)) = (current_model_id, record.acpx.as_mut()) {
                acpx.current_model_id = Some(current_model_id);
            }
        }
        Err(err) => {
            log::warn!("[acp] failed to apply requested model \"{requested_model}\": {err}");
        }
    }
}

/// Ports the `AcpClient` construction + `connectAndLoadSession` half of
/// acpx's `withConnectedSession`/manager.ts session setup.
pub(super) async fn spawn_connected_session(
    options: &AcpRuntimeOptions,
    mut record: SessionRecord,
    resume_policy: SessionResumePolicy,
) -> Result<Arc<ConnectedSession>, AcpRuntimeError> {
    let agent_command = record.agent_command.clone();
    let mut parts = resolve_agent_program(&agent_command, None).map_err(|err| {
        wrap_err(
            AcpRuntimeErrorCode::BackendMissing,
            "failed to resolve agent command",
            err,
        )
    })?;
    parts.args =
        crate::agent_command::resolve_gemini_command_args(&parts.command, &parts.args).await;
    let is_gemini = crate::agent_command::is_gemini_acp_command(&parts.command, &parts.args);
    let is_devin = crate::agent_command::is_devin_acp_command(&parts.command, &parts.args);
    if crate::agent_command::is_copilot_acp_command(&parts.command, &parts.args) {
        crate::agent_command::ensure_copilot_acp_support(&parts.command)
            .await
            .map_err(|err| {
                wrap_err(
                    AcpRuntimeErrorCode::BackendUnsupportedControl,
                    "copilot ACP stdio mode unsupported",
                    err,
                )
            })?;
    }
    let cwd = PathBuf::from(&record.cwd);
    let session_env = session_options_from_record(&record).and_then(|o| o.env);
    let env = build_agent_environment(
        std::env::vars(),
        options.auth_credentials.as_ref(),
        session_env.as_ref(),
        cfg!(windows),
    );

    let (notifications_tx, notifications_rx) = smol::channel::unbounded();
    // Gap 20: filesystem/terminal client-operation events flow through this
    // channel to the active turn task (drained there, persisted via
    // `record_client_operation` + streamed as `AcpRuntimeEvent::ClientOperation`).
    let (operations_tx, operations_rx) = smol::channel::unbounded::<ClientOperation>();
    let fs_operations_tx = operations_tx.clone();
    let filesystem = Arc::new(
        FilesystemHandlers::new(
            &cwd,
            options.permission_mode,
            options.non_interactive_permissions,
            options.on_permission_request.clone(),
        )
        .map_err(|err| {
            wrap_err(
                AcpRuntimeErrorCode::SessionInitFailed,
                "failed to sandbox session cwd",
                err,
            )
        })?
        .with_on_operation(Arc::new(move |op| {
            let _ = fs_operations_tx.try_send(op);
        })),
    );
    let terminal = Arc::new(
        TerminalManager::new(TerminalManagerOptions {
            cwd: cwd.clone(),
            permission_mode: options.permission_mode,
            non_interactive_policy: options.non_interactive_permissions,
            handler: options.on_permission_request.clone(),
            kill_grace: None,
        })
        .with_on_operation(Arc::new(move |op| {
            let _ = operations_tx.try_send(op);
        })),
    );
    let handlers = ClientRequestHandlers {
        filesystem: Some(filesystem),
        terminal: Some(terminal),
        permission: PermissionRequestWiring {
            mode: options.permission_mode,
            non_interactive_policy: options.non_interactive_permissions,
            handler: options.on_permission_request.clone(),
            policy: options.permission_policy.clone(),
            on_escalation: options.on_permission_escalation.clone(),
            stats: Default::default(),
        },
        notifications: Some(notifications_tx),
    };

    let client = AcpClient::spawn(SpawnAgentOptions {
        program: &parts.command,
        args: &parts.args,
        cwd: &cwd,
        env: &env,
        client_name: "boltz-acpx".to_string(),
        terminal: options.terminal,
        is_gemini,
        is_devin,
        handlers,
        auth_credentials: options.auth_credentials.clone(),
    })
    .await
    .map_err(|err| {
        wrap_err(
            AcpRuntimeErrorCode::SessionInitFailed,
            "failed to spawn ACP agent",
            err,
        )
    })?;

    record.protocol_version = Some(i64::from(client.init_response().protocol_version.as_u16()));
    record.agent_capabilities = Some(client.init_response().agent_capabilities.clone());

    let conversation = conversation_from_record(&record);

    let connect_result = connect_and_load_session(
        &client,
        &mut record,
        &conversation,
        &options.mcp_servers,
        resume_policy,
        options.timeout_ms.map(Duration::from_millis),
    )
    .await
    .map_err(|err| {
        wrap_err(
            AcpRuntimeErrorCode::SessionInitFailed,
            "failed to connect ACP session",
            err,
        )
    })?;

    // Gap 16: `connect_and_load_session`'s `session/load`/`session/resume`
    // RPC may have triggered the agent to replay historical `session/update`
    // notifications onto `notifications_rx` before this `ConnectedSession`
    // (and therefore any live consumer) exists — drain and discard whatever
    // accumulated so a turn started right after `ensure_session` doesn't see
    // stale replay content ahead of its own live updates. See
    // `connected_session::drain_replay_notifications`'s docs for why a
    // synchronous channel-empty check is sufficient here (no wall-clock
    // idle wait needed).
    let suppressed_replay_updates = if connect_result.resumed {
        drain_replay_notifications(&notifications_rx)
    } else {
        0
    };

    // Gap 12: apply a caller-requested model to the live connection
    // unconditionally (resumed or freshly created alike), persisting the
    // resulting current-model-id onto the record before it's saved below.
    apply_requested_model_if_advertised(&client, &mut record).await;

    record.pid = client.state().last_known_pid;
    record.agent_started_at = Some(client.state().agent_started_at.to_rfc3339());
    record.closed = false;
    record.closed_at = None;
    record.last_used_at = iso_now();

    options
        .session_store
        .save(record.clone())
        .await
        .map_err(|err| {
            wrap_err(
                AcpRuntimeErrorCode::SessionInitFailed,
                "failed to persist session record",
                err,
            )
        })?;

    Ok(ConnectedSession::new(
        client,
        connect_result.session_id,
        record,
        conversation,
        notifications_rx,
        operations_rx,
        options.mcp_servers.clone(),
        options.prompt_queue_capacity,
        suppressed_replay_updates,
    ))
}
