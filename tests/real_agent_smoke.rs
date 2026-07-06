#![cfg(feature = "test-support")]
//! Real-agent smoke test: drives the crate end to end against the ACTUAL
//! `@agentclientprotocol/claude-agent-acp` adapter (resolved from the
//! built-in registry as `npx -y @agentclientprotocol/claude-agent-acp@^…`),
//! NOT the synthetic fake agent.
//!
//! Every test here is `#[ignore]` by default: it needs network access (npx
//! downloads the adapter on first run) and a working Claude Code auth on the
//! host, so it is intentionally non-hermetic and excluded from the default
//! `cargo test` run (which stays deterministic on the fake agent). This is
//! the "real agent acceptance" check — run it manually to prove the crate
//! actually talks to real Claude:
//!
//! ```sh
//! cargo test -p boltz-acpx --features test-support --test real_agent_smoke \
//!     -- --ignored --nocapture
//! ```

use boltz_acpx::runtime::public::{
    AcpRuntime, AcpRuntimeEnsureInput, AcpRuntimeEvent, AcpRuntimeOptions, AcpRuntimePromptMode,
    AcpRuntimeSessionMode, AcpRuntimeTurnInput, AcpRuntimeTurnResult, BuiltInAgentRegistry,
};
use boltz_acpx::session::persistence::FileAcpSessionStore;
use boltz_acpx::session::store_options::AcpFileSessionStoreOptions;
use boltz_acpx::types::{NonInteractivePermissionPolicy, PermissionMode};
use futures::StreamExt;

/// Builds a runtime with the *default* built-in agent registry (no test
/// override), so `agent: "claude"` resolves to the real
/// `@agentclientprotocol/claude-agent-acp` adapter command.
fn real_runtime(state_dir: &std::path::Path, cwd: &std::path::Path) -> AcpRuntime {
    let options = AcpRuntimeOptions {
        cwd: cwd.to_path_buf(),
        session_store: FileAcpSessionStore::new(AcpFileSessionStoreOptions::new(state_dir)),
        agent_registry: std::sync::Arc::new(BuiltInAgentRegistry::new(None)),
        mcp_servers: Vec::new(),
        // ApproveAll so a tool-using turn never blocks on an interactive
        // permission prompt during the smoke test.
        permission_mode: PermissionMode::ApproveAll,
        non_interactive_permissions: NonInteractivePermissionPolicy::Deny,
        // Real model + first-run npx download can be slow.
        timeout_ms: Some(180_000),
        probe_agent: None,
        verbose: false,
        terminal: false,
        on_permission_request: None,
        permission_policy: None,
        on_permission_escalation: None,
        auth_credentials: None,
        prompt_queue_capacity: None,
    };
    AcpRuntime::new(options)
}

#[test]
#[ignore = "requires network (npx) + Claude Code auth; run with --ignored"]
fn real_claude_initialize_session_and_prompt() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = real_runtime(state_dir.path(), cwd.path());

        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "real-smoke".to_string(),
                agent: "claude".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect("ensure_session against real claude-agent-acp should succeed");

        assert!(
            handle.backend_session_id.is_some(),
            "real agent should assign a backend session id"
        );

        let mut turn = runtime
            .start_turn(AcpRuntimeTurnInput {
                handle,
                text: "Reply with exactly one word: pong".to_string(),
                attachments: Vec::new(),
                mode: AcpRuntimePromptMode::Prompt,
                request_id: "req-real-1".to_string(),
                timeout_ms: None,
            })
            .await;

        let mut events = turn.events();
        let mut text = String::new();
        while let Some(event) = events.next().await {
            if let AcpRuntimeEvent::TextDelta { text: delta, .. } = &event {
                text.push_str(delta);
            }
        }

        let result = turn.result().await;
        assert!(
            matches!(result, AcpRuntimeTurnResult::Completed { .. }),
            "expected Completed from real claude, got {result:?}"
        );
        assert!(
            !text.trim().is_empty(),
            "expected some agent text back from real claude, got empty"
        );
        eprintln!("real claude replied: {text:?}");
    });
}
