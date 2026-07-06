//! Ports `others/acpx/src/runtime/public/probe.ts`: spawn a throwaway agent
//! process, run the ACP handshake, and report whether the embedded runtime
//! is usable end to end — this crate's version of `acpx doctor`.

use std::path::Path;

use crate::agent_command::resolve_agent_program;
use crate::auth_env::build_agent_environment;
use crate::client::{AcpClient, SpawnAgentOptions};

use super::contract::AcpRuntimeOptions;

/// Ports `RuntimeHealthReport`.
#[derive(Debug, Clone)]
pub struct RuntimeHealthReport {
    pub ok: bool,
    pub message: String,
    pub details: Vec<String>,
}

/// Ports `probeRuntime`: resolves `options.probe_agent` (falling back to
/// [`crate::agent_command::DEFAULT_AGENT_NAME`]), spawns it, completes the
/// `initialize` handshake, then shuts it down — never touches session
/// persistence or the connected-session engine, matching acpx's own
/// probe scope.
pub async fn probe_runtime(options: &AcpRuntimeOptions) -> RuntimeHealthReport {
    let agent_name = options
        .probe_agent
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(crate::agent_command::DEFAULT_AGENT_NAME);
    let agent_command = options.agent_registry.resolve(agent_name);

    let mut parts = match resolve_agent_program(&agent_command, None) {
        Ok(parts) => parts,
        Err(err) => {
            return RuntimeHealthReport {
                ok: false,
                message: "embedded ACP runtime probe failed".to_string(),
                details: vec![
                    format!("agent={agent_name}"),
                    format!("command={agent_command}"),
                    format!("cwd={}", options.cwd.display()),
                    err.to_string(),
                ],
            };
        }
    };
    parts.args =
        crate::agent_command::resolve_gemini_command_args(&parts.command, &parts.args).await;
    let is_gemini = crate::agent_command::is_gemini_acp_command(&parts.command, &parts.args);
    let is_devin = crate::agent_command::is_devin_acp_command(&parts.command, &parts.args);
    if crate::agent_command::is_copilot_acp_command(&parts.command, &parts.args) {
        if let Err(err) = crate::agent_command::ensure_copilot_acp_support(&parts.command).await {
            return RuntimeHealthReport {
                ok: false,
                message: "embedded ACP runtime probe failed".to_string(),
                details: vec![
                    format!("agent={agent_name}"),
                    format!("command={agent_command}"),
                    format!("cwd={}", options.cwd.display()),
                    err.to_string(),
                ],
            };
        }
    }

    let env = build_agent_environment(
        std::env::vars(),
        options.auth_credentials.as_ref(),
        None,
        cfg!(windows),
    );
    let client = AcpClient::spawn(SpawnAgentOptions {
        program: &parts.command,
        args: &parts.args,
        cwd: Path::new(&options.cwd),
        env: &env,
        client_name: "boltz-acpx-probe".to_string(),
        terminal: options.terminal,
        is_gemini,
        is_devin,
        handlers: Default::default(),
        auth_credentials: options.auth_credentials.clone(),
    })
    .await;

    match client {
        Ok(client) => {
            let mut details = vec![
                format!("agent={agent_name}"),
                format!("command={agent_command}"),
                format!("cwd={}", options.cwd.display()),
            ];
            details.push(format!(
                "protocolVersion={:?}",
                client.init_response().protocol_version
            ));
            client.shutdown().await;
            RuntimeHealthReport {
                ok: true,
                message: "embedded ACP runtime ready".to_string(),
                details,
            }
        }
        Err(err) => RuntimeHealthReport {
            ok: false,
            message: "embedded ACP runtime probe failed".to_string(),
            details: vec![
                format!("agent={agent_name}"),
                format!("command={agent_command}"),
                format!("cwd={}", options.cwd.display()),
                err.to_string(),
            ],
        },
    }
}
