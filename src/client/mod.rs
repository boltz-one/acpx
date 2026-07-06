//! Public `AcpClient` handle: ties together spawn, transport, handshake,
//! shutdown, and lifecycle state into the single object Phase 3/4/6 build
//! on. Ports the surface of `others/acpx/src/acp/client.ts` this phase owns
//! (handshake/session-new/shutdown) — prompt-queue state is Phase 6's.

pub mod handlers;
pub mod handshake;
pub mod shutdown;
pub mod spawn;
pub mod state;
pub mod transport;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    CancelNotification, CloseSessionRequest, CloseSessionResponse, InitializeResponse,
    LoadSessionRequest, LoadSessionResponse, McpServer, Meta, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, ResumeSessionRequest, ResumeSessionResponse,
    SessionConfigId, SessionConfigValueId, SessionId, SessionModeId, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
};
use agent_client_protocol::{Agent, ConnectionTo, Error as AcpRpcError};

use crate::error::{AcpError, Result};
use crate::error_normalization::normalize_agent_error;
use crate::session_control_errors::{SessionControlMethod, maybe_wrap_session_control_error};
pub use handlers::ClientRequestHandlers;
use handshake::{RunningConnection, spawn_and_initialize};
use spawn::{SpawnOptions, spawn_agent_process};
use state::{AgentExitInfo, ClientState};
use transport::take_transport;

/// Ports the wrapping half of acpx's `client.ts` `setSessionMode`/
/// `setSessionConfigOption` `catch` blocks (gap 11): prefer
/// [`maybe_wrap_session_control_error`]'s clearer, context-carrying message
/// over the generic [`normalize_agent_error`] fallback whenever the raw RPC
/// error looks like "the adapter doesn't implement this / rejected this
/// value".
fn wrap_or_normalize_control_error(
    method: SessionControlMethod,
    error: AcpRpcError,
    context: Option<String>,
) -> AcpError {
    if let Some(message) = maybe_wrap_session_control_error(method, &error, context.as_deref()) {
        return AcpError::Other(anyhow::anyhow!(message));
    }
    normalize_agent_error(error, String::new())
}

/// Everything needed to spawn one ACP agent and complete its handshake.
pub struct SpawnAgentOptions<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub cwd: &'a Path,
    pub env: &'a HashMap<String, String>,
    pub client_name: String,
    pub terminal: bool,
    /// Whether `program`/`args` resolve to a Gemini CLI `--acp` invocation
    /// (see [`crate::agent_command::is_gemini_acp_command`]). When true,
    /// the `initialize` handshake races
    /// [`crate::agent_command::gemini_quirks::resolve_gemini_acp_startup_timeout_ms`]
    /// instead of waiting indefinitely, surfacing
    /// [`AcpError::GeminiAcpStartupTimeout`] on expiry (matching acpx's
    /// Gemini-specific startup-timeout handling).
    pub is_gemini: bool,
    /// Whether `program`/`args` resolve to a Devin ACP invocation (see
    /// [`crate::agent_command::is_devin_acp_command`]). When true, the
    /// advertised `clientInfo`/`clientCapabilities` are swapped for
    /// Devin's Windsurf compatibility identity (see
    /// `client::handshake`'s module docs).
    pub is_devin: bool,
    /// Phase 4's filesystem/terminal/permission handlers and `session/update`
    /// notification sink, registered on the connection before the
    /// `initialize` handshake runs. Defaults (all `None`/empty) are fine for
    /// a handshake-only client (e.g. [`crate::runtime::public::probe::probe_runtime`]).
    pub handlers: ClientRequestHandlers,
    /// Gap 3/24: app-supplied auth-method credential map, used to select an
    /// `authenticate` method after the `initialize` handshake succeeds.
    /// `None` = no app-supplied credentials (ambient `ACP_AUTH_*` env is
    /// still consulted).
    pub auth_credentials: Option<HashMap<String, String>>,
}

/// A running ACP agent subprocess plus its live connection handle. Owns no
/// prompt-queue state (see module docs) — that's [`crate::client`]'s
/// consumers in later phases, not this struct.
pub struct AcpClient {
    child: crate::util::process::Child,
    connection: ConnectionTo<Agent>,
    init_response: InitializeResponse,
    state: ClientState,
    /// The resolved `program + args` command line, kept so [`Self::shutdown`]
    /// can apply the per-agent stdin-close→SIGTERM grace period (gap 22, e.g.
    /// Qoder's longer grace) via `resolve_agent_close_after_stdin_end_ms`.
    agent_command: String,
    /// Shared handle to the permission-decision counters the connection's
    /// `session/request_permission` RPC handler increments (gap 25). Cloned
    /// out of `handlers.permission.stats` before the handlers move into the
    /// handshake, so [`Self::permission_stats`] can read the same counters.
    permission_stats: state::PermissionStatsHandle,
    _task: smol::Task<std::result::Result<(), AcpRpcError>>,
    shutdown_tx: Option<futures::channel::oneshot::Sender<()>>,
}

impl AcpClient {
    /// Spawns the agent subprocess and performs the ACP `initialize`
    /// handshake. Ports the combined effect of acpx's `client-process.ts`
    /// spawn + `client.ts`'s `initializeAgentConnection`.
    pub async fn spawn(options: SpawnAgentOptions<'_>) -> Result<Self> {
        let mut child = spawn_agent_process(SpawnOptions {
            program: options.program,
            args: options.args,
            cwd: options.cwd,
            env: options.env,
        })?;
        let pid = child.id();
        let transport = take_transport(&mut child)?;
        // Clone the shared stats handle before `options.handlers` moves into
        // `spawn_and_initialize`, so this `AcpClient` reads the same counters
        // the RPC handler increments.
        let permission_stats = options.handlers.permission.stats.clone();
        // Reconstruct the command line (gap 22) so `shutdown` can resolve the
        // per-agent stdin-close grace period.
        let agent_command = std::iter::once(options.program.to_string())
            .chain(options.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");

        let init_outcome = if options.is_gemini {
            let timeout = crate::agent_command::resolve_gemini_acp_startup_timeout_ms();
            match crate::control::with_timeout(
                spawn_and_initialize(
                    transport,
                    options.client_name.clone(),
                    options.terminal,
                    options.is_devin,
                    options.handlers,
                    options.auth_credentials,
                ),
                Some(timeout),
            )
            .await
            {
                Ok(inner) => inner,
                Err(AcpError::Timeout(_)) => {
                    // The agent hasn't finished `initialize` within Gemini's
                    // startup budget (usually stuck on interactive OAuth) —
                    // kill the hung subprocess rather than leaking it, then
                    // surface a Gemini-specific diagnostic.
                    let _ = child.kill();
                    let message = crate::agent_command::gemini_quirks::build_gemini_acp_startup_timeout_message(
                        options.program,
                    )
                    .await;
                    Err(AcpError::GeminiAcpStartupTimeout(message))
                }
                Err(other) => Err(other),
            }
        } else {
            spawn_and_initialize(
                transport,
                options.client_name.clone(),
                options.terminal,
                options.is_devin,
                options.handlers,
                options.auth_credentials,
            )
            .await
        };

        let RunningConnection {
            connection,
            init_response,
            task,
            shutdown_tx,
        } = init_outcome.map_err(|err| attach_command(err, options.program))?;

        Ok(Self {
            child,
            connection,
            init_response,
            state: ClientState::new(pid),
            agent_command,
            permission_stats,
            _task: task,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub fn init_response(&self) -> &InitializeResponse {
        &self.init_response
    }

    pub fn state(&self) -> &ClientState {
        &self.state
    }

    /// Snapshot of this connection's permission-request counters (gap 25).
    /// Ports acpx's `getPermissionStats()`.
    pub fn permission_stats(&self) -> state::PermissionStats {
        self.permission_stats.lock().clone()
    }

    pub fn connection(&self) -> &ConnectionTo<Agent> {
        &self.connection
    }

    /// Ports the `session/new` half of acpx's `client.ts`.
    pub async fn session_new(
        &self,
        cwd: PathBuf,
        mcp_servers: Vec<McpServer>,
    ) -> Result<NewSessionResponse> {
        self.session_new_with_meta(cwd, mcp_servers, None).await
    }

    /// Gap 10: like [`Self::session_new`], but also attaches an outbound
    /// `_meta` object to the `session/new` request (e.g. the
    /// `_meta.claudeCode.options` block built by
    /// [`crate::runtime::engine::session_options::build_claude_code_options_meta`]).
    /// A separate method rather than extending `session_new`'s signature so
    /// existing call sites (and `client_lifecycle.rs`'s tests, out of this
    /// phase's scope) keep compiling unchanged.
    pub async fn session_new_with_meta(
        &self,
        cwd: PathBuf,
        mcp_servers: Vec<McpServer>,
        meta: Option<Meta>,
    ) -> Result<NewSessionResponse> {
        let request = NewSessionRequest::new(cwd)
            .mcp_servers(mcp_servers)
            .meta(meta);
        self.connection
            .send_request(request)
            .block_task()
            .await
            .map_err(|err| normalize_agent_error(err, String::new()))
    }

    /// Ports the `session/load` half of acpx's `client.ts` (used by
    /// [`crate::runtime::engine::reconnect`] when the agent advertises
    /// `agentCapabilities.loadSession`).
    pub async fn session_load(
        &self,
        session_id: SessionId,
        cwd: PathBuf,
        mcp_servers: Vec<McpServer>,
    ) -> Result<LoadSessionResponse> {
        let request = LoadSessionRequest::new(session_id, cwd).mcp_servers(mcp_servers);
        self.connection
            .send_request(request)
            .block_task()
            .await
            .map_err(|err| normalize_agent_error(err, String::new()))
    }

    /// Ports the `session/resume` half of acpx's `client.ts`.
    pub async fn session_resume(
        &self,
        session_id: SessionId,
        cwd: PathBuf,
        mcp_servers: Vec<McpServer>,
    ) -> Result<ResumeSessionResponse> {
        let request = ResumeSessionRequest::new(session_id, cwd).mcp_servers(mcp_servers);
        self.connection
            .send_request(request)
            .block_task()
            .await
            .map_err(|err| normalize_agent_error(err, String::new()))
    }

    /// Sends `session/prompt` and awaits the agent's terminal response.
    pub async fn prompt(&self, request: PromptRequest) -> Result<PromptResponse> {
        self.connection
            .send_request(request)
            .block_task()
            .await
            .map_err(|err| normalize_agent_error(err, String::new()))
    }

    /// Sends the `session/cancel` notification (fire-and-forget per ACP;
    /// the agent replies to the still-pending `session/prompt` request with
    /// `stopReason: cancelled`, it does not reply to this notification).
    pub fn cancel_session(&self, session_id: SessionId) -> Result<()> {
        self.connection
            .send_notification(CancelNotification::new(session_id))
            .map_err(|err| AcpError::Other(anyhow::anyhow!("failed to send session/cancel: {err}")))
    }

    /// Ports `AcpClient.setSessionMode`. Gap 11: a rejected/unsupported
    /// `session/set_mode` is wrapped via [`maybe_wrap_session_control_error`]
    /// (acpx's exact `for mode "{mode_id}"` context) before falling back to
    /// the generic normalized error.
    pub async fn set_session_mode(
        &self,
        session_id: SessionId,
        mode_id: SessionModeId,
    ) -> Result<SetSessionModeResponse> {
        let context = format!("for mode \"{}\"", mode_id.0);
        let request = SetSessionModeRequest::new(session_id, mode_id);
        crate::jsonrpc_gap::send_set_session_mode(&self.connection, request)
            .await
            .map_err(|err| {
                wrap_or_normalize_control_error(SessionControlMethod::SetMode, err, Some(context))
            })
    }

    /// Ports `AcpClient.setSessionConfigOption`. Gap 11: wraps rejected/
    /// unsupported `session/set_config_option` errors the same way, with
    /// acpx's `for "{config_id}"="{value}"` context.
    pub async fn set_session_config_option(
        &self,
        session_id: SessionId,
        config_id: SessionConfigId,
        value: SessionConfigValueId,
    ) -> Result<SetSessionConfigOptionResponse> {
        // Gap 26: remap the config id for legacy Zed codex-acp invocations
        // (e.g. `thought_level` -> `reasoning_effort`) before sending — the
        // single choke point every config-option set (interactive,
        // model-application, replay) flows through.
        let config_id = SessionConfigId::new(crate::agent_command::resolve_compatible_config_id(
            &self.agent_command,
            config_id.0.as_ref(),
        ));
        let context = format!("for \"{}\"=\"{}\"", config_id.0, value.0);
        let request = SetSessionConfigOptionRequest::new(session_id, config_id, value);
        crate::jsonrpc_gap::send_set_session_config_option(&self.connection, request)
            .await
            .map_err(|err| {
                wrap_or_normalize_control_error(
                    SessionControlMethod::SetConfigOption,
                    err,
                    Some(context),
                )
            })
    }

    /// Gap 9: ports the `session/close` half of acpx's `client.ts`
    /// `closeSession`. Returns the raw normalized [`Result`] so
    /// [`crate::runtime::engine::manager::queue_control`]'s `close()` can
    /// classify resource-not-found/unsupported failures itself (best-effort
    /// RPC — see that module's docs).
    pub async fn session_close(&self, session_id: SessionId) -> Result<CloseSessionResponse> {
        let request = CloseSessionRequest::new(session_id);
        self.connection
            .send_request(request)
            .block_task()
            .await
            .map_err(|err| normalize_agent_error(err, String::new()))
    }

    /// Gracefully shuts down the agent process (SIGTERM grace, then
    /// SIGKILL escalation), tearing down the background connection task
    /// first so it doesn't race the process teardown.
    pub async fn shutdown(mut self) -> AgentExitInfo {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let pid = self.state.last_known_pid.unwrap_or(0);
        // Gap 22: apply the per-agent stdin-close→SIGTERM grace period
        // (e.g. Qoder's longer window) resolved from the command line.
        let info = shutdown::shutdown_agent_process_for_agent_command(
            &mut self.child,
            pid,
            &self.agent_command,
        )
        .await;
        self.state.record_exit(info.clone());
        info
    }
}

/// `spawn_and_initialize` doesn't know the resolved command string (only
/// `client/mod.rs` does); this fills it into `AgentStartup` errors for a
/// useful message.
fn attach_command(err: AcpError, command: &str) -> AcpError {
    match err {
        AcpError::AgentStartup {
            exit_code,
            signal,
            stderr_summary,
            ..
        } => AcpError::AgentStartup {
            command: command.to_string(),
            exit_code,
            signal,
            stderr_summary,
        },
        other => other,
    }
}
