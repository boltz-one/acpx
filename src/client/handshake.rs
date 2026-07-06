//! ACP `initialize` handshake + background connection driver. Ports the
//! handshake half of `others/acpx/src/acp/client.ts`
//! (`initializeProtocolConnection`/`initializeAgentConnection`), plus (new
//! in Phase 4) registering the agent-initiated RPC handlers Phase 3 defined
//! (filesystem, terminal, permission) and the `session/update` notification
//! sink the runtime engine consumes.
//!
//! The Rust SDK's connection model is fundamentally different from acpx's:
//! `Client.builder().connect_with(transport, main_fn)` drives the whole
//! connection lifetime through one async closure, rather than handing back
//! a long-lived connection object. To still expose a long-lived,
//! poke-at-any-time [`AcpClient`](super::AcpClient) handle (this phase's
//! Requirement 1), `spawn_and_initialize` runs `connect_with` on a
//! background `smol` task (ADR-2) whose closure clones out a
//! `ConnectionTo<Agent>` (cheap and `Send`, per the SDK's own internal use
//! of `.clone()`) through a oneshot channel immediately after `initialize`
//! succeeds, then parks on a second oneshot until told to shut down.
//!
//! Handler registration must happen on the `Builder` *before*
//! `connect_with` runs (the SDK has no post-connect "add a handler" API —
//! see `agent_client_protocol::Builder::on_receive_request`'s docs), which
//! is why [`ClientRequestHandlers`] is threaded in here rather than
//! attached by the runtime engine after the fact.

use std::collections::HashMap;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, ClientCapabilities, CreateTerminalRequest, CreateTerminalResponse,
    FileSystemCapabilities, Implementation, InitializeRequest, InitializeResponse,
    KillTerminalRequest, KillTerminalResponse, Meta, ReadTextFileRequest, ReadTextFileResponse,
    ReleaseTerminalRequest, ReleaseTerminalResponse, RequestPermissionRequest,
    RequestPermissionResponse, SessionNotification, TerminalOutputRequest, TerminalOutputResponse,
    WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Error as AcpRpcError};
use futures::channel::oneshot;

use super::handlers::ClientRequestHandlers;
use super::transport::AgentByteStreams;
use crate::auth_env::{read_env_credential, resolve_configured_auth_credential};
use crate::error::{AcpError, Result};
use crate::version::crate_version;

/// Ports `DEVIN_COMPATIBILITY_CLIENT_NAME`: Devin's server-side ACP
/// precondition only passes for a recognized Windsurf IDE client identity,
/// so this crate reports itself as Windsurf (not its real name/version)
/// when talking to a Devin ACP agent. A documented interop workaround (see
/// `others/acpx/src/acp/client.ts`), not a security concern.
const DEVIN_COMPATIBILITY_CLIENT_NAME: &str = "windsurf";

/// Ports `DEFAULT_DEVIN_COMPATIBILITY_CLIENT_VERSION`: the embedded
/// Windsurf IDE version bundled with Devin Desktop 3.1.7, the first
/// version acpx locally verified passes Devin's server-side ACP
/// precondition. Overridable via `ACPX_DEVIN_WINDSURF_VERSION`.
const DEFAULT_DEVIN_COMPATIBILITY_CLIENT_VERSION: &str = "1.110.1";

/// A running background connection: the live request-sending handle plus
/// the join handle for the `connect_with` task and the signal to stop it.
pub struct RunningConnection {
    pub connection: ConnectionTo<Agent>,
    pub init_response: InitializeResponse,
    pub task: smol::Task<std::result::Result<(), AcpRpcError>>,
    pub shutdown_tx: oneshot::Sender<()>,
}

/// Ports `resolveClientCapabilities`: advertises this crate's real
/// fs/terminal capabilities, plus (when `is_devin`) the
/// `cognition.ai/requestDiagnostics` `_meta` flag Devin's ACP agent expects
/// from a Windsurf-identified client.
fn client_capabilities(terminal: bool, is_devin: bool) -> ClientCapabilities {
    let base = ClientCapabilities::new()
        .fs(FileSystemCapabilities::new()
            .read_text_file(true)
            .write_text_file(true))
        .terminal(terminal);
    if is_devin {
        base.meta(devin_client_capabilities_meta())
    } else {
        base
    }
}

fn devin_client_capabilities_meta() -> Meta {
    let mut meta = Meta::new();
    meta.insert(
        "cognition.ai/requestDiagnostics".to_string(),
        serde_json::Value::Bool(true),
    );
    meta
}

/// Ports `resolveClientInfo`: reports this crate's real identity, unless
/// talking to a Devin agent (see [`DEVIN_COMPATIBILITY_CLIENT_NAME`]'s
/// docs above).
fn client_info(client_name: String, is_devin: bool) -> Implementation {
    if is_devin {
        Implementation::new(
            DEVIN_COMPATIBILITY_CLIENT_NAME,
            resolve_devin_windsurf_version(),
        )
    } else {
        Implementation::new(client_name, crate_version())
    }
}

fn resolve_devin_windsurf_version() -> String {
    resolve_devin_windsurf_version_from(
        std::env::var("ACPX_DEVIN_WINDSURF_VERSION").ok().as_deref(),
    )
}

fn resolve_devin_windsurf_version_from(raw: Option<&str>) -> String {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_DEVIN_COMPATIBILITY_CLIENT_VERSION)
        .to_string()
}

/// Converts this crate's [`AcpError`] into the wire-level
/// [`AcpRpcError`] sent back to the agent when a Phase 3 handler fails.
fn rpc_error_from(err: &AcpError) -> AcpRpcError {
    let code = i32::try_from(err.json_rpc_code()).unwrap_or(-32603);
    AcpRpcError::new(code, err.to_string())
}

/// Gap 3: ports `authenticateIfRequired`/`selectAuthMethod`. If the agent's
/// `initialize` response advertised any auth methods, pick the first one for
/// which a credential resolves — either from the app-supplied
/// `auth_credentials` map or the ambient `ACP_AUTH_*` environment — and send
/// the `authenticate` RPC to select it (the credential itself already
/// reaches the agent via its subprocess environment, see
/// [`crate::auth_env::build_agent_environment`]). When no credential
/// resolves for any advertised method, proceed unauthenticated (Requirement
/// 4 / plan Unresolved Questions #6 decided default): this crate has no
/// `authPolicy: "fail"` option, and letting the agent reject the first real
/// RPC yields a clearer error than a silent hang. An `authenticate` RPC that
/// *does* run but fails is a real connection failure and propagates.
async fn authenticate_if_required(
    cx: &ConnectionTo<Agent>,
    init_response: &InitializeResponse,
    auth_credentials: Option<&HashMap<String, String>>,
) -> std::result::Result<(), AcpRpcError> {
    if init_response.auth_methods.is_empty() {
        return Ok(());
    }
    let selected = init_response.auth_methods.iter().find(|method| {
        let id = method.id().0.as_ref();
        resolve_configured_auth_credential(id, auth_credentials).is_some()
            || read_env_credential(id).is_some()
    });
    let Some(method) = selected else {
        log::debug!(
            "agent advertised {} auth method(s) but no credential resolved; proceeding unauthenticated",
            init_response.auth_methods.len()
        );
        return Ok(());
    };
    cx.send_request(AuthenticateRequest::new(method.id().clone()))
        .block_task()
        .await
        .map(|_response| ())
}

/// Performs the `initialize` handshake over `transport` and leaves the
/// connection running in the background. `client_name` is this crate's
/// advertised `clientInfo.name` (e.g. `"boltz-acpx"` or an app-provided
/// override); `terminal` advertises `terminal/*` capability support;
/// `is_devin` swaps the advertised `clientInfo`/`clientCapabilities` for
/// Devin's Windsurf compatibility identity (see this module's docs above);
/// `handlers` wires up the agent-initiated RPCs (see module docs);
/// `auth_credentials` is the app-supplied auth-method credential map used to
/// select an `authenticate` method after `initialize` (gap 3, see
/// [`authenticate_if_required`]).
pub async fn spawn_and_initialize(
    transport: AgentByteStreams,
    client_name: String,
    terminal: bool,
    is_devin: bool,
    handlers: ClientRequestHandlers,
    auth_credentials: Option<HashMap<String, String>>,
) -> Result<RunningConnection> {
    let (ready_tx, ready_rx) = oneshot::channel::<
        std::result::Result<(ConnectionTo<Agent>, InitializeResponse), AcpRpcError>,
    >();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let task = smol::spawn(async move {
        let filesystem = handlers.filesystem.clone();
        let terminal_manager = handlers.terminal.clone();
        let permission = handlers.permission.clone();
        let notifications = handlers.notifications.clone();

        Client
            .builder()
            .on_receive_request(
                move |req: ReadTextFileRequest,
                      responder: agent_client_protocol::Responder<ReadTextFileResponse>,
                      cx: ConnectionTo<Agent>| {
                    let filesystem = filesystem.clone();
                    async move {
                        cx.spawn(async move {
                            let Some(filesystem) = filesystem else {
                                return responder.respond_with_error(AcpRpcError::new(
                                    -32601,
                                    "fs/read_text_file is not configured for this client",
                                ));
                            };
                            let result: std::result::Result<ReadTextFileResponse, AcpError> =
                                filesystem.read_text_file(req).await;
                            match result {
                                Ok(response) => responder.respond(response),
                                Err(err) => responder.respond_with_error(rpc_error_from(&err)),
                            }
                        })?;
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let filesystem = handlers.filesystem.clone();
                    move |req: WriteTextFileRequest,
                          responder: agent_client_protocol::Responder<WriteTextFileResponse>,
                          cx: ConnectionTo<Agent>| {
                        let filesystem = filesystem.clone();
                        async move {
                            cx.spawn(async move {
                                let Some(filesystem) = filesystem else {
                                    return responder.respond_with_error(AcpRpcError::new(
                                        -32601,
                                        "fs/write_text_file is not configured for this client",
                                    ));
                                };
                                let result: std::result::Result<WriteTextFileResponse, AcpError> =
                                    filesystem.write_text_file(req).await;
                                match result {
                                    Ok(response) => responder.respond(response),
                                    Err(err) => responder.respond_with_error(rpc_error_from(&err)),
                                }
                            })?;
                            Ok(())
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                move |req: RequestPermissionRequest,
                      responder: agent_client_protocol::Responder<RequestPermissionResponse>,
                      cx: ConnectionTo<Agent>| {
                    let permission = permission.clone();
                    async move {
                        cx.spawn(async move {
                            // Gap 1/2: use the full decision-tree function so
                            // the caller-supplied `policy` actually applies and
                            // the escalation audit event is surfaced (the
                            // response-only wrapper hardcoded `None` for policy
                            // and discarded the escalation).
                            let resolved =
                                crate::permissions::resolve_permission_request_with_details(
                                    &req,
                                    permission.mode,
                                    permission.non_interactive_policy,
                                    permission.policy.as_ref(),
                                    permission.handler.as_deref(),
                                )
                                .await;
                            match resolved {
                                Ok(resolved) => {
                                    let escalation = resolved.escalation;
                                    let response = resolved.response;
                                    // Gap 25: count the resolved decision.
                                    let class = crate::permissions::classify_permission_decision(
                                        &req, &response,
                                    );
                                    permission.stats.lock().record(class);
                                    // Gap 2 (ADR-8): fire-and-forget escalation
                                    // audit callback, panic-isolated so a
                                    // misbehaving caller can't poison this RPC
                                    // response path.
                                    if let (Some(cb), Some(event)) =
                                        (&permission.on_escalation, escalation)
                                    {
                                        let cb = cb.clone();
                                        let _ = std::panic::catch_unwind(
                                            std::panic::AssertUnwindSafe(|| cb(event)),
                                        );
                                    }
                                    responder.respond(response)
                                }
                                Err(err) => {
                                    permission.stats.lock().record_error();
                                    responder.respond_with_error(rpc_error_from(&err))
                                }
                            }
                        })?;
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let terminal_manager = terminal_manager.clone();
                    move |req: CreateTerminalRequest,
                          responder: agent_client_protocol::Responder<CreateTerminalResponse>,
                          cx: ConnectionTo<Agent>| {
                        let terminal_manager = terminal_manager.clone();
                        async move {
                            cx.spawn(async move {
                                let Some(terminal_manager) = terminal_manager else {
                                    return responder.respond_with_error(AcpRpcError::new(
                                        -32601,
                                        "terminal/create is not configured for this client",
                                    ));
                                };
                                let result: std::result::Result<CreateTerminalResponse, AcpError> =
                                    terminal_manager.create_terminal(req).await;
                                match result {
                                    Ok(response) => responder.respond(response),
                                    Err(err) => responder.respond_with_error(rpc_error_from(&err)),
                                }
                            })?;
                            Ok(())
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let terminal_manager = terminal_manager.clone();
                    move |req: TerminalOutputRequest,
                          responder: agent_client_protocol::Responder<TerminalOutputResponse>,
                          cx: ConnectionTo<Agent>| {
                        let terminal_manager = terminal_manager.clone();
                        async move {
                            cx.spawn(async move {
                                let Some(terminal_manager) = terminal_manager else {
                                    return responder.respond_with_error(AcpRpcError::new(
                                        -32601,
                                        "terminal/output is not configured for this client",
                                    ));
                                };
                                let result: std::result::Result<TerminalOutputResponse, AcpError> =
                                    terminal_manager.terminal_output(req).await;
                                match result {
                                    Ok(response) => responder.respond(response),
                                    Err(err) => responder.respond_with_error(rpc_error_from(&err)),
                                }
                            })?;
                            Ok(())
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let terminal_manager = terminal_manager.clone();
                    move |req: ReleaseTerminalRequest,
                          responder: agent_client_protocol::Responder<ReleaseTerminalResponse>,
                          cx: ConnectionTo<Agent>| {
                        let terminal_manager = terminal_manager.clone();
                        async move {
                            cx.spawn(async move {
                                let Some(terminal_manager) = terminal_manager else {
                                    return responder.respond_with_error(AcpRpcError::new(
                                        -32601,
                                        "terminal/release is not configured for this client",
                                    ));
                                };
                                let result: std::result::Result<ReleaseTerminalResponse, AcpError> =
                                    terminal_manager.release_terminal(req).await;
                                match result {
                                    Ok(response) => responder.respond(response),
                                    Err(err) => responder.respond_with_error(rpc_error_from(&err)),
                                }
                            })?;
                            Ok(())
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let terminal_manager = terminal_manager.clone();
                    move |req: KillTerminalRequest,
                          responder: agent_client_protocol::Responder<KillTerminalResponse>,
                          cx: ConnectionTo<Agent>| {
                        let terminal_manager = terminal_manager.clone();
                        async move {
                            cx.spawn(async move {
                                let Some(terminal_manager) = terminal_manager else {
                                    return responder.respond_with_error(AcpRpcError::new(
                                        -32601,
                                        "terminal/kill is not configured for this client",
                                    ));
                                };
                                let result: std::result::Result<KillTerminalResponse, AcpError> =
                                    terminal_manager.kill_terminal(req).await;
                                match result {
                                    Ok(response) => responder.respond(response),
                                    Err(err) => responder.respond_with_error(rpc_error_from(&err)),
                                }
                            })?;
                            Ok(())
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let terminal_manager = terminal_manager.clone();
                    move |req: WaitForTerminalExitRequest,
                          responder: agent_client_protocol::Responder<
                        WaitForTerminalExitResponse,
                    >,
                          cx: ConnectionTo<Agent>| {
                        let terminal_manager = terminal_manager.clone();
                        async move {
                            cx.spawn(async move {
                                let Some(terminal_manager) = terminal_manager else {
                                    return responder.respond_with_error(AcpRpcError::new(
                                        -32601,
                                        "terminal/wait_for_exit is not configured for this client",
                                    ));
                                };
                                let result: std::result::Result<
                                    WaitForTerminalExitResponse,
                                    AcpError,
                                > = terminal_manager.wait_for_terminal_exit(req).await;
                                match result {
                                    Ok(response) => responder.respond(response),
                                    Err(err) => responder.respond_with_error(rpc_error_from(&err)),
                                }
                            })?;
                            Ok(())
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                move |notif: SessionNotification, _cx: ConnectionTo<Agent>| {
                    let notifications = notifications.clone();
                    async move {
                        if let Some(sender) = notifications {
                            // Best-effort: a full/closed receiver means no
                            // one is listening for this session's live
                            // updates right now (e.g. between turns), which
                            // is not itself a connection-level error.
                            let _ = sender.try_send(notif);
                        }
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
                let request = InitializeRequest::new(ProtocolVersion::LATEST)
                    .client_capabilities(client_capabilities(terminal, is_devin))
                    .client_info(client_info(client_name, is_devin));

                match cx.send_request(request).block_task().await {
                    Ok(response) => {
                        // Gap 3: select an auth method before handing the
                        // connection to the caller. A failure here is a
                        // connection failure (surfaced on `ready_tx`).
                        if let Err(auth_error) =
                            authenticate_if_required(&cx, &response, auth_credentials.as_ref())
                                .await
                        {
                            let _ = ready_tx.send(Err(auth_error.clone()));
                            return Err(auth_error);
                        }
                        let _ = ready_tx.send(Ok((cx.clone(), response)));
                        // Park until told to shut down; connect_with tears
                        // the transport down once this closure returns.
                        let _ = shutdown_rx.await;
                        Ok(())
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.clone()));
                        Err(error)
                    }
                }
            })
            .await
    });

    match ready_rx.await {
        Ok(Ok((connection, init_response))) => Ok(RunningConnection {
            connection,
            init_response,
            task,
            shutdown_tx,
        }),
        Ok(Err(rpc_error)) => Err(AcpError::Other(anyhow::anyhow!(
            "ACP initialize failed: {} (code {})",
            rpc_error.message,
            i32::from(rpc_error.code)
        ))),
        Err(_) => {
            // The background task ended (agent process likely exited)
            // before sending anything on `ready_tx`; surface whatever the
            // task future itself returned, if it has already finished.
            let outcome = task.await;
            Err(AcpError::AgentStartup {
                command: client_name_unused(),
                exit_code: None,
                signal: None,
                stderr_summary: outcome.err().map(|e| e.message),
            })
        }
    }
}

// The `AgentStartup` variant's `command` field isn't meaningful here (the
// caller already knows the command it spawned); `client/mod.rs` overwrites
// it with the real command before surfacing the error to callers. Kept as a
// tiny named helper (rather than an inline placeholder) so the intent reads
// clearly at the call site above.
fn client_name_unused() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_capabilities_advertise_fs_and_terminal() {
        let caps = client_capabilities(true, false);
        assert!(caps.fs.read_text_file);
        assert!(caps.fs.write_text_file);
        assert!(caps.terminal);
        assert!(caps.meta.is_none());
    }

    #[test]
    fn client_capabilities_can_omit_terminal() {
        let caps = client_capabilities(false, false);
        assert!(!caps.terminal);
    }

    #[test]
    fn devin_client_capabilities_advertise_request_diagnostics_meta() {
        let caps = client_capabilities(true, true);
        let meta = caps.meta.expect("devin capabilities must carry _meta");
        assert_eq!(
            meta.get("cognition.ai/requestDiagnostics"),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn client_info_reports_real_identity_by_default() {
        let info = client_info("boltz-acpx".to_string(), false);
        assert_eq!(info.name, "boltz-acpx");
        assert_eq!(info.version, crate_version());
    }

    #[test]
    fn client_info_spoofs_windsurf_identity_for_devin() {
        let info = client_info("boltz-acpx".to_string(), true);
        assert_eq!(info.name, DEVIN_COMPATIBILITY_CLIENT_NAME);
        assert_eq!(info.version, DEFAULT_DEVIN_COMPATIBILITY_CLIENT_VERSION);
    }

    #[test]
    fn devin_windsurf_version_defaults_when_env_unset_or_blank() {
        assert_eq!(
            resolve_devin_windsurf_version_from(None),
            DEFAULT_DEVIN_COMPATIBILITY_CLIENT_VERSION
        );
        assert_eq!(
            resolve_devin_windsurf_version_from(Some("   ")),
            DEFAULT_DEVIN_COMPATIBILITY_CLIENT_VERSION
        );
    }

    #[test]
    fn devin_windsurf_version_honors_env_override() {
        assert_eq!(resolve_devin_windsurf_version_from(Some("2.0.0")), "2.0.0");
    }
}
