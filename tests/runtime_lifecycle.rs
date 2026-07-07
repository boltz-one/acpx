#![cfg(feature = "test-support")]
//! Phase 4 integration tests: drive the real `AcpRuntime` public contract
//! end to end against the real fake ACP agent binary (extended in this
//! phase to support `session/prompt` + `session/update` and
//! `session/resume`) — no mocks, per Success Criteria.

use std::collections::HashMap;

use boltz_acpx::permissions::PermissionPolicy;
use boltz_acpx::runtime::engine::session_options::SessionAgentOptions;
use boltz_acpx::runtime::public::{
    AcpRuntime, AcpRuntimeEnsureInput, AcpRuntimeErrorCode, AcpRuntimeEvent, AcpRuntimeOptions,
    AcpRuntimePromptMode, AcpRuntimeSessionMode, AcpRuntimeTurnInput, AcpRuntimeTurnResult,
    BuiltInAgentRegistry, decode_runtime_handle_state,
};
use boltz_acpx::session::persistence::FileAcpSessionStore;
use boltz_acpx::session::store_options::AcpFileSessionStoreOptions;
use boltz_acpx::types::{NonInteractivePermissionPolicy, PermissionMode};
use futures::StreamExt;

fn fake_agent_path() -> &'static str {
    env!("CARGO_BIN_EXE_acp-fake-agent")
}

fn test_runtime(state_dir: &std::path::Path, cwd: &std::path::Path) -> AcpRuntime {
    let overrides = HashMap::from([("test-agent".to_string(), fake_agent_path().to_string())]);
    let options = AcpRuntimeOptions {
        cwd: cwd.to_path_buf(),
        session_store: FileAcpSessionStore::new(AcpFileSessionStoreOptions::new(state_dir)),
        agent_registry: std::sync::Arc::new(BuiltInAgentRegistry::new(Some(overrides))),
        mcp_servers: Vec::new(),
        permission_mode: PermissionMode::ApproveAll,
        non_interactive_permissions: NonInteractivePermissionPolicy::Deny,
        timeout_ms: Some(5_000),
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

/// Success Criteria #1: ensure_session -> start_turn -> drain events to
/// completion -> await result -> see `Completed`.
#[test]
fn full_lifecycle_ensure_session_and_prompt_turn() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "full-lifecycle".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect("ensure_session should succeed against the real fake agent");

        assert!(handle.backend_session_id.is_some());

        let mut turn = runtime
            .start_turn(AcpRuntimeTurnInput {
                handle,
                text: "hello".to_string(),
                attachments: Vec::new(),
                mode: AcpRuntimePromptMode::Prompt,
                request_id: "req-1".to_string(),
                timeout_ms: None,
            })
            .await;

        let mut events = turn.events();
        let mut saw_text_delta = false;
        while let Some(event) = events.next().await {
            if let AcpRuntimeEvent::TextDelta { text, .. } = &event {
                assert_eq!(text, "hello from fake agent");
                saw_text_delta = true;
            }
        }
        assert!(
            saw_text_delta,
            "expected a text_delta event from the live session/update"
        );

        let result = turn.result().await;
        assert!(
            matches!(result, AcpRuntimeTurnResult::Completed { .. }),
            "expected Completed, got {result:?}"
        );
    });
}

/// Success Criteria #2: kill the fake agent process, `ensure_session` again
/// with the same key, confirm the runtime transparently reconnects (backend
/// session id unchanged, proving `session/resume` was used rather than a
/// silent fresh `session/new`) rather than a silent state loss or panic.
#[test]
fn reconnect_after_agent_crash_resumes_backend_session() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([(
                "ACP_FAKE_AGENT_EXIT_AFTER_MS".to_string(),
                "150".to_string(),
            )])),
            ..Default::default()
        };

        let first_handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "crash-recovery".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("initial ensure_session should succeed");
        let original_backend_session_id = first_handle
            .backend_session_id
            .clone()
            .expect("handle should carry a backend session id");

        // Let the fake agent's self-destruct timer fire (simulating a crash
        // while this process — the embedding app — keeps running).
        smol::Timer::after(std::time::Duration::from_millis(500)).await;

        let second_handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "crash-recovery".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect("ensure_session after a crash must reconnect, not fail silently");

        assert_eq!(
            second_handle.backend_session_id.as_deref(),
            Some(original_backend_session_id.as_str()),
            "expected the reconnected handle to resume the original backend session id \
             (proves session/resume was used, not a silent fresh session/new)"
        );
    });
}

/// Gap 1/2/25 real call-path proof: a policy `escalate` rule + escalation
/// callback configured on `AcpRuntimeOptions` must actually reach the
/// handshake-registered `session/request_permission` handler, so a real
/// agent-issued permission request escalates and fires the audit callback.
/// Before the fix, `handshake.rs` hardcoded `policy: None` and discarded the
/// escalation — this drives the full runtime path (options -> manager_spawn
/// -> wiring -> handshake closure) with the fake agent issuing a real
/// `request_permission` for an `execute`-kind tool.
#[test]
fn escalate_policy_surfaces_escalation_over_real_runtime_path() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let overrides = HashMap::from([("test-agent".to_string(), fake_agent_path().to_string())]);

        let (tx, rx) = smol::channel::unbounded::<Option<String>>();
        let policy = PermissionPolicy {
            escalate: vec!["execute".to_string()],
            ..Default::default()
        };
        let options = AcpRuntimeOptions {
            cwd: cwd.path().to_path_buf(),
            session_store: FileAcpSessionStore::new(AcpFileSessionStoreOptions::new(
                state_dir.path(),
            )),
            agent_registry: std::sync::Arc::new(BuiltInAgentRegistry::new(Some(overrides))),
            mcp_servers: Vec::new(),
            permission_mode: PermissionMode::ApproveAll,
            non_interactive_permissions: NonInteractivePermissionPolicy::Deny,
            timeout_ms: Some(5_000),
            probe_agent: None,
            verbose: false,
            terminal: false,
            on_permission_request: None,
            permission_policy: Some(policy),
            on_permission_escalation: Some(std::sync::Arc::new(move |event| {
                let _ = tx.try_send(event.matched_rule.clone());
            })),
            auth_credentials: None,
            prompt_queue_capacity: None,
        };
        let runtime = AcpRuntime::new(options);

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([(
                "ACP_FAKE_AGENT_REQUEST_PERMISSION".to_string(),
                "Execute: rm -rf /".to_string(),
            )])),
            ..Default::default()
        };

        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "escalation".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("ensure_session should succeed");

        let mut turn = runtime
            .start_turn(AcpRuntimeTurnInput {
                handle,
                text: "please run a command".to_string(),
                attachments: Vec::new(),
                mode: AcpRuntimePromptMode::Prompt,
                request_id: "req-esc".to_string(),
                timeout_ms: None,
            })
            .await;

        // Drive the turn to completion so the agent-issued permission request
        // is processed by the handshake handler.
        let mut events = turn.events();
        while events.next().await.is_some() {}
        let _ = turn.result().await;

        // Blocks until the escalation callback fires (unbounded channel), so
        // there is no race with turn completion.
        let matched = rx
            .recv()
            .await
            .expect("escalation callback should have fired for the escalate rule");
        assert_eq!(
            matched.as_deref(),
            Some("execute"),
            "escalation event should carry the matched policy rule"
        );
    });
}

/// Gap 6: a prompt RPC that times out AFTER the agent already replied (via
/// `session/update`) must report `Completed`, not a hard `TIMEOUT` failure —
/// ports acpx's `hasAgentReplyAfterPrompt` fallback. The fake agent emits its
/// update immediately, then delays the terminal `session/prompt` response
/// past the (short) per-turn timeout.
#[test]
fn timeout_with_late_reply_reports_completed_not_failed() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([(
                "ACP_FAKE_AGENT_PROMPT_DELAY_MS".to_string(),
                "3000".to_string(),
            )])),
            ..Default::default()
        };
        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "late-reply".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("ensure_session should succeed");

        let mut turn = runtime
            .start_turn(AcpRuntimeTurnInput {
                handle,
                text: "hi".to_string(),
                attachments: Vec::new(),
                mode: AcpRuntimePromptMode::Prompt,
                request_id: "req-late".to_string(),
                timeout_ms: Some(500),
            })
            .await;

        let mut events = turn.events();
        while events.next().await.is_some() {}
        let result = turn.result().await;
        assert!(
            matches!(result, AcpRuntimeTurnResult::Completed { .. }),
            "a timeout after the agent already replied should be Completed, got {result:?}"
        );
    });
}

/// Gap 6 regression guard: a genuinely silent timeout (agent sends NO update
/// and never responds in time) must still report `Failed{code:TIMEOUT}` — the
/// reply-check must not turn every timeout into a false success.
#[test]
fn timeout_without_any_reply_still_reports_timeout() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([
                (
                    "ACP_FAKE_AGENT_PROMPT_UPDATE_COUNT".to_string(),
                    "0".to_string(),
                ),
                (
                    "ACP_FAKE_AGENT_PROMPT_DELAY_MS".to_string(),
                    "3000".to_string(),
                ),
            ])),
            ..Default::default()
        };
        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "silent-timeout".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("ensure_session should succeed");

        let mut turn = runtime
            .start_turn(AcpRuntimeTurnInput {
                handle,
                text: "hi".to_string(),
                attachments: Vec::new(),
                mode: AcpRuntimePromptMode::Prompt,
                request_id: "req-silent".to_string(),
                timeout_ms: Some(500),
            })
            .await;

        let mut events = turn.events();
        while events.next().await.is_some() {}
        match turn.result().await {
            AcpRuntimeTurnResult::Failed { error } => {
                assert_eq!(error.code.as_deref(), Some("TIMEOUT"));
            }
            other => panic!("expected Failed TIMEOUT for a silent timeout, got {other:?}"),
        }
    });
}

/// Gap 5: the `Load` acquisition path (agent advertises `loadSession` but not
/// `resume`) must run end to end on reconnect. First connect creates a fresh
/// session; after the agent dies, reconnect goes through `session/load`
/// (not `session/new`), so the backend session id is preserved.
#[test]
fn reconnect_via_load_path_preserves_backend_session() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([(
                "ACP_FAKE_AGENT_SESSION_CAPABILITIES".to_string(),
                "load".to_string(),
            )])),
            ..Default::default()
        };
        let first = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "load-path".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("initial ensure_session should succeed");
        let original = first
            .backend_session_id
            .clone()
            .expect("handle should carry a backend session id");

        // A fresh runtime over the same store has no in-memory session, so
        // ensure_session MUST reconnect from the persisted record (no live
        // reuse). The persisted capabilities advertise loadSession-only, so
        // acquisition deterministically takes the Load path; session/load
        // preserves the backend session id.
        let runtime2 = test_runtime(state_dir.path(), cwd.path());
        let second = runtime2
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "load-path".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect("reconnect via session/load must succeed");

        assert_eq!(
            second.backend_session_id.as_deref(),
            Some(original.as_str()),
            "the Load path preserves the backend session id"
        );
    });
}

/// Gap 5: under `Persistent` mode (SameSessionOnly resume policy), a
/// `session/resume` failure must surface `SessionResumeRequired` rather than
/// silently creating a fresh session — losing a persistent session's
/// continuity would be a data-integrity regression. (The
/// fallback-to-fresh classification for the AllowNew policy is covered by
/// `reconnect`'s `should_fallback_to_new_session` unit tests.)
#[test]
fn reconnect_resume_failure_requires_same_session_under_persistent() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([(
                // Persisted so the RECONNECT agent's session/resume fails.
                "ACP_FAKE_AGENT_RESUME_ERROR_CODE".to_string(),
                "-32601".to_string(),
            )])),
            ..Default::default()
        };
        runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "resume-required".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("initial ensure_session should succeed");

        // Fresh runtime over the same store -> reconnect from the persisted
        // record (no live reuse). The reconnect agent's session/resume errors;
        // under Persistent/SameSessionOnly this must NOT fall back.
        let runtime2 = test_runtime(state_dir.path(), cwd.path());
        let err = runtime2
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "resume-required".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect_err("a resume failure under Persistent mode must not silently succeed");
        let text = format!("{err:?} {}", err.message);
        assert!(
            text.contains("could not be resumed"),
            "expected a SessionResumeRequired diagnostic, got: {text}"
        );
    });
}

/// Gap 4: a Claude ACP command whose `session/new` hangs past the
/// Claude-specific creation timeout must surface `ClaudeAcpSessionCreateTimeout`
/// (a distinct diagnostic), not a generic timeout. Uses a fake agent invoked
/// under a `claude-agent-acp` arg (so `is_claude_acp_command` matches) plus a
/// short env-overridden timeout and a delayed `session/new`.
#[test]
fn claude_session_create_timeout_reports_dedicated_error() {
    // Only this test reads this env var; set_var affects the whole process.
    unsafe {
        std::env::set_var("ACPX_CLAUDE_ACP_SESSION_CREATE_TIMEOUT_MS", "300");
    }

    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        // Register a "claude-like" agent whose command line carries a
        // `claude-agent-acp` arg so `is_claude_acp_command` matches; the fake
        // agent binary ignores the extra arg.
        let claude_cmd = format!("{} claude-agent-acp", fake_agent_path());
        let overrides = HashMap::from([("claude-fake".to_string(), claude_cmd)]);
        let options = AcpRuntimeOptions {
            cwd: cwd.path().to_path_buf(),
            session_store: FileAcpSessionStore::new(AcpFileSessionStoreOptions::new(
                state_dir.path(),
            )),
            agent_registry: std::sync::Arc::new(BuiltInAgentRegistry::new(Some(overrides))),
            mcp_servers: Vec::new(),
            permission_mode: PermissionMode::ApproveAll,
            non_interactive_permissions: NonInteractivePermissionPolicy::Deny,
            timeout_ms: Some(10_000),
            probe_agent: None,
            verbose: false,
            terminal: false,
            on_permission_request: None,
            permission_policy: None,
            on_permission_escalation: None,
            auth_credentials: None,
            prompt_queue_capacity: None,
        };
        let runtime = AcpRuntime::new(options);

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([(
                "ACP_FAKE_AGENT_SESSION_NEW_DELAY_MS".to_string(),
                "3000".to_string(),
            )])),
            ..Default::default()
        };
        let err = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "claude-timeout".to_string(),
                agent: "claude-fake".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect_err("a hung Claude session/new must time out");
        let text = format!("{err:?} {}", err.message);
        assert!(
            text.contains("timed out"),
            "expected a Claude-specific session-create timeout diagnostic, got: {text}"
        );
    });

    unsafe {
        std::env::remove_var("ACPX_CLAUDE_ACP_SESSION_CREATE_TIMEOUT_MS");
    }
}

/// Exercises `get_status`, `set_mode`, and `cancel` against the real fake
/// agent — not called out as a separate Success Criterion, but cheap
/// insurance that the rest of the public contract's methods (not just
/// `ensure_session`/`start_turn`) are wired correctly end to end.
#[test]
fn status_mode_and_cancel_round_trip_against_real_agent() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "status-mode-cancel".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect("ensure_session should succeed");

        let status = runtime
            .get_status(&handle)
            .await
            .expect("get_status should succeed for a connected session");
        assert_eq!(
            status.backend_session_id.as_deref(),
            handle.backend_session_id.as_deref()
        );

        runtime
            .set_mode(&handle, "plan")
            .await
            .expect("the fake agent accepts any session/set_mode call");

        // No prompt is in flight, so `cancel` should report success as a
        // no-op rather than erroring.
        runtime
            .cancel(&handle, Some("test cleanup"))
            .await
            .expect("cancel with no active prompt should not error");

        let capabilities = runtime.get_capabilities(None).await;
        assert!(!capabilities.controls.is_empty());
    });
}

/// Gap 9(a): `close(discard_persistent_state: true)` against a fake agent
/// that advertises `sessionCapabilities.close` must actually send
/// `session/close` — proven via the fake agent's RPC log, not just "close()
/// didn't error".
#[test]
fn close_with_discard_sends_session_close_when_capability_advertised() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());
        let rpc_log = tempfile::NamedTempFile::new().unwrap();

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([
                (
                    "ACP_FAKE_AGENT_ADVERTISE_CLOSE".to_string(),
                    "1".to_string(),
                ),
                (
                    "ACP_FAKE_AGENT_RPC_LOG".to_string(),
                    rpc_log.path().to_string_lossy().into_owned(),
                ),
            ])),
            ..Default::default()
        };
        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "close-with-capability".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("ensure_session should succeed");

        runtime
            .close(&handle, "test cleanup", true)
            .await
            .expect("close with discard should succeed");

        let log = std::fs::read_to_string(rpc_log.path()).unwrap_or_default();
        assert!(
            log.lines().any(|line| line == "session/close"),
            "expected session/close to have been sent, RPC log was: {log:?}"
        );
    });
}

/// Gap 9(b): the same `close(discard_persistent_state: true)` against a
/// fake agent that does NOT advertise `sessionCapabilities.close` must
/// neither attempt the RPC nor error.
#[test]
fn close_with_discard_skips_session_close_when_capability_absent() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());
        let rpc_log = tempfile::NamedTempFile::new().unwrap();

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([(
                "ACP_FAKE_AGENT_RPC_LOG".to_string(),
                rpc_log.path().to_string_lossy().into_owned(),
            )])),
            ..Default::default()
        };
        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "close-without-capability".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("ensure_session should succeed");

        runtime
            .close(&handle, "test cleanup", true)
            .await
            .expect("close with discard must not error even without the capability");

        let log = std::fs::read_to_string(rpc_log.path()).unwrap_or_default();
        assert!(
            !log.lines().any(|line| line == "session/close"),
            "session/close must not be sent when the agent doesn't advertise the capability, RPC log was: {log:?}"
        );
    });
}

/// Gap 11(c): a `set_mode` the fake agent rejects must surface
/// `maybe_wrap_session_control_error`'s wrapped, context-carrying message —
/// not the bare JSON-RPC error text.
#[test]
fn rejected_set_mode_surfaces_wrapped_control_error() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([(
                "ACP_FAKE_AGENT_SET_MODE_ERROR_CODE".to_string(),
                "-32601".to_string(),
            )])),
            ..Default::default()
        };
        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "rejected-set-mode".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("ensure_session should succeed");

        let err = runtime
            .set_mode(&handle, "plan")
            .await
            .expect_err("a rejected session/set_mode must surface an error");
        let text = format!("{err:?} {}", err.message);
        assert!(
            text.contains("session/set_mode") && text.contains("for mode \"plan\""),
            "expected the wrapped session-control message (with mode context), got: {text}"
        );
        assert!(
            !text.contains("method not found"),
            "expected the wrapped message, not the raw JSON-RPC error text, got: {text}"
        );
    });
}

/// Gap 12(d): a session created with `session_options.model` set must
/// actually issue the model-setting RPC against the live agent, and the
/// record's current-model-id must reflect the fake agent's response — not
/// just that some RPC was sent.
#[test]
fn session_options_model_is_applied_to_the_live_connection() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());
        let rpc_log = tempfile::NamedTempFile::new().unwrap();

        let session_options = SessionAgentOptions {
            model: Some("gpt-5".to_string()),
            env: Some(HashMap::from([
                ("ACP_FAKE_AGENT_MODEL_CONFIG".to_string(), "1".to_string()),
                (
                    "ACP_FAKE_AGENT_RPC_LOG".to_string(),
                    rpc_log.path().to_string_lossy().into_owned(),
                ),
            ])),
            ..Default::default()
        };
        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "model-applied".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("ensure_session should succeed");

        let log = std::fs::read_to_string(rpc_log.path()).unwrap_or_default();
        assert!(
            log.lines().any(|line| line == "session/set_config_option"),
            "expected session/set_config_option to have been sent, RPC log was: {log:?}"
        );

        let status = runtime
            .get_status(&handle)
            .await
            .expect("get_status should succeed");
        assert_eq!(
            status.models.and_then(|m| m.current_model_id).as_deref(),
            Some("gpt-5"),
            "the record's current-model-id should reflect the fake agent's response"
        );
    });
}

/// `AcpRuntime::set_model` round-trip against the real fake agent: a session
/// that advertises a `model`-category config option (`ACP_FAKE_AGENT_MODEL_CONFIG=1`)
/// must, after `set_model`, actually send `session/set_config_option` and
/// have `get_status().models.current_model_id` reflect the fake agent's
/// echoed-back response — not just that the call didn't error.
#[test]
fn set_model_round_trips_through_get_status() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());
        let rpc_log = tempfile::NamedTempFile::new().unwrap();

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([
                ("ACP_FAKE_AGENT_MODEL_CONFIG".to_string(), "1".to_string()),
                (
                    "ACP_FAKE_AGENT_RPC_LOG".to_string(),
                    rpc_log.path().to_string_lossy().into_owned(),
                ),
            ])),
            ..Default::default()
        };
        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "set-model-round-trip".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("ensure_session should succeed");

        let initial_status = runtime
            .get_status(&handle)
            .await
            .expect("get_status should succeed for a connected session");
        assert_eq!(
            initial_status
                .models
                .as_ref()
                .and_then(|m| m.current_model_id.as_deref()),
            Some("default-model"),
            "the fake agent should advertise its default current model at session creation"
        );

        runtime
            .set_model(&handle, "gpt-5", None)
            .await
            .expect("the fake agent accepts any session/set_config_option call for model");

        let log = std::fs::read_to_string(rpc_log.path()).unwrap_or_default();
        assert!(
            log.lines().any(|line| line == "session/set_config_option"),
            "expected session/set_config_option to have been sent, RPC log was: {log:?}"
        );

        let status = runtime
            .get_status(&handle)
            .await
            .expect("get_status should succeed");
        assert_eq!(
            status.models.and_then(|m| m.current_model_id).as_deref(),
            Some("gpt-5"),
            "set_model should update the record's current-model-id to reflect the fake agent's response"
        );
    });
}

/// Gap 15(e): a fresh session whose `session/new` response omits
/// `configOptions` after an earlier connection had some must clear the
/// stale value, not carry it over.
#[test]
fn fresh_session_after_earlier_connection_clears_stale_config_options() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());
        // A path inside a tempdir that does NOT exist yet — `NamedTempFile`
        // would create the file immediately, defeating
        // `is_first_call_for_marker`'s "does the marker exist" check.
        let marker_dir = tempfile::tempdir().unwrap();
        let marker_path = marker_dir.path().join("first-call-marker");
        // The fake agent doesn't advertise resume/load, so every
        // `ensure_session` for this key goes through a fresh `session/new`.
        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([
                (
                    "ACP_FAKE_AGENT_SESSION_CAPABILITIES".to_string(),
                    "none".to_string(),
                ),
                (
                    "ACP_FAKE_AGENT_OMIT_CONFIG_AFTER_FIRST_FILE".to_string(),
                    marker_path.to_string_lossy().into_owned(),
                ),
            ])),
            ..Default::default()
        };
        let first = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "stale-config-options".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Oneshot,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("initial ensure_session should succeed");
        let first_status = runtime
            .get_status(&first)
            .await
            .expect("get_status should succeed");
        assert!(
            first_status
                .models
                .and_then(|m| m.current_model_id)
                .is_some(),
            "the first connection's session/new advertised a model config option"
        );

        // A fresh runtime over the same store has no in-memory session, so
        // ensure_session reconnects from the persisted record; the "none"
        // capabilities profile forces the CreateFresh path again, and the
        // marker file makes this second fake-agent process omit
        // configOptions entirely.
        let runtime2 = test_runtime(state_dir.path(), cwd.path());
        let second = runtime2
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "stale-config-options".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Oneshot,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect("reconnect should succeed");

        let second_status = runtime2
            .get_status(&second)
            .await
            .expect("get_status should succeed");
        let second_current_model_id = second_status.models.and_then(|m| m.current_model_id);
        assert!(
            second_current_model_id.is_none(),
            "a fresh session/new that omits configOptions must clear the stale model state, got {second_current_model_id:?}"
        );
    });
}

/// Gap 13: `get_capabilities(Some(handle))` against a real, connected
/// session that advertised config options via `session/new` must surface
/// those options' ids as `config_option_keys` — proving the manager's
/// live-record wiring (reads `connected.record.acpx.config_options`
/// in-process, no extra I/O per this phase's ADR), not just that the pure
/// key-extraction logic works in isolation.
#[test]
fn get_capabilities_with_handle_reflects_live_session_config_options() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([(
                "ACP_FAKE_AGENT_MODEL_CONFIG".to_string(),
                "1".to_string(),
            )])),
            ..Default::default()
        };
        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "capabilities-with-config-options".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("ensure_session should succeed");

        let capabilities = runtime.get_capabilities(Some(&handle)).await;
        assert_eq!(
            capabilities.config_option_keys.as_deref(),
            Some(["model".to_string()].as_slice()),
            "expected the fake agent's advertised \"model\" config option id to surface via get_capabilities"
        );

        // No handle at all is still the backward-compatible static list
        // with no `config_option_keys`.
        let bare_capabilities = runtime.get_capabilities(None).await;
        assert!(bare_capabilities.config_option_keys.is_none());
    });
}

/// Gap 34: `ensure_session` must reject a blank `session_key`/`agent` as
/// its first statements, with acpx's exact error messages, before any
/// agent-registry/session-store work happens.
#[test]
fn ensure_session_rejects_blank_session_key_and_agent() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let blank_key_err = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "   ".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect_err("a blank (whitespace-only) session key must be rejected");
        assert_eq!(blank_key_err.code, AcpRuntimeErrorCode::SessionInitFailed);
        assert_eq!(blank_key_err.message, "ACP session key is required.");

        let blank_agent_err = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "some-key".to_string(),
                agent: "".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect_err("a blank agent id must be rejected");
        assert_eq!(blank_agent_err.code, AcpRuntimeErrorCode::SessionInitFailed);
        assert_eq!(blank_agent_err.message, "ACP agent id is required.");

        // Both blank: the session-key check runs first, matching acpx's
        // ordering (`sessionName` validated before `agent`).
        let both_blank_err = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "".to_string(),
                agent: "".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect_err("both blank must be rejected");
        assert_eq!(both_blank_err.message, "ACP session key is required.");
    });
}

/// Gap 34: leading/trailing whitespace on an otherwise-valid
/// `session_key`/`agent` must be trimmed before use — the resulting
/// handle's `session_key` is the trimmed value, not the raw input.
#[test]
fn ensure_session_trims_whitespace_from_session_key_and_agent() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "  padded-key  ".to_string(),
                agent: "  test-agent  ".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect("a session key/agent with surrounding whitespace should still succeed");

        assert_eq!(handle.session_key, "padded-key");
    });
}

/// Gap 14: `handle_for`'s `runtime_session_name` must be the versioned
/// opaque-encoded string `write_handle_state`/`encode_runtime_handle_state`
/// produce, and it must round-trip through `decode_runtime_handle_state`
/// back to the agent/cwd/mode `ensure_session` actually used — not a raw
/// `session_key` copy.
#[test]
fn handle_for_runtime_session_name_round_trips_through_decode() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "handle-state-round-trip".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect("ensure_session should succeed");

        assert_ne!(
            handle.runtime_session_name, handle.session_key,
            "runtime_session_name must be the opaque-encoded string, not a raw session_key copy"
        );

        let decoded = decode_runtime_handle_state(&handle.runtime_session_name)
            .expect("a handle_for-produced runtime_session_name must decode successfully");
        assert_eq!(decoded.name, "handle-state-round-trip");
        assert_eq!(decoded.agent, "test-agent");
        assert_eq!(decoded.cwd, cwd.path().to_string_lossy());
        assert_eq!(decoded.mode, AcpRuntimeSessionMode::Persistent);
    });
}

/// Gap 20 real call-path proof: a filesystem operation the agent triggers
/// mid-turn (`fs/read_text_file`) must surface as an
/// `AcpRuntimeEvent::ClientOperation` on the turn's event stream — proving
/// the `on_operation` callback -> per-session channel -> turn-drain ->
/// event/`record_client_operation` wiring, not just the handler-level
/// callback. (Before this, `filesystem.rs` wrongly documented client
/// operations as CLI-only and the runtime wiring did not exist.)
#[test]
fn client_operation_events_stream_from_agent_filesystem_reads() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(cwd.path().join("probe.txt"), "hello from disk").unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path());

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([
                (
                    "ACP_FAKE_AGENT_READ_FILE".to_string(),
                    cwd.path().join("probe.txt").to_string_lossy().into_owned(),
                ),
                // Small delay so the agent-issued fs read is processed and
                // its client_operation event streamed before the terminal
                // prompt response closes the turn.
                (
                    "ACP_FAKE_AGENT_PROMPT_DELAY_MS".to_string(),
                    "400".to_string(),
                ),
            ])),
            ..Default::default()
        };
        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "client-op".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("ensure_session should succeed");

        let mut turn = runtime
            .start_turn(AcpRuntimeTurnInput {
                handle,
                text: "please read the probe file".to_string(),
                attachments: Vec::new(),
                mode: AcpRuntimePromptMode::Prompt,
                request_id: "req-op".to_string(),
                timeout_ms: None,
            })
            .await;

        let mut events = turn.events();
        let mut saw_read_op = false;
        while let Some(event) = events.next().await {
            if let AcpRuntimeEvent::ClientOperation { method, .. } = &event {
                if method.contains("read_text_file") {
                    saw_read_op = true;
                }
            }
        }
        let _ = turn.result().await;
        assert!(
            saw_read_op,
            "expected a ClientOperation event for the agent-issued fs/read_text_file"
        );
    });
}
