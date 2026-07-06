#![cfg(feature = "test-support")]
//! Phase 6 integration tests: drive the real `AcpRuntime` public contract
//! (queue-wired `start_turn`) against the real fake-agent binary — no
//! mocks, per Success Criteria. Each test targets one Success Criterion /
//! Requirement 9 sub-case from
//! `plans/20260705-1718-acpx-to-acp-crate-port/phase-06-prompt-queueing-cancellation.md`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use boltz_acpx::runtime::engine::session_options::SessionAgentOptions;
use boltz_acpx::runtime::public::{
    AcpRuntime, AcpRuntimeEnsureInput, AcpRuntimeEvent, AcpRuntimeOptions, AcpRuntimePromptMode,
    AcpRuntimeSessionMode, AcpRuntimeTurnInput, AcpRuntimeTurnResult, BuiltInAgentRegistry,
};
use boltz_acpx::session::persistence::FileAcpSessionStore;
use boltz_acpx::session::store_options::AcpFileSessionStoreOptions;
use boltz_acpx::types::{NonInteractivePermissionPolicy, PermissionMode};
use futures::StreamExt;

fn fake_agent_path() -> &'static str {
    env!("CARGO_BIN_EXE_acp-fake-agent")
}

fn test_runtime(
    state_dir: &std::path::Path,
    cwd: &std::path::Path,
    prompt_queue_capacity: Option<usize>,
) -> std::sync::Arc<AcpRuntime> {
    let overrides = HashMap::from([("test-agent".to_string(), fake_agent_path().to_string())]);
    let options = AcpRuntimeOptions {
        cwd: cwd.to_path_buf(),
        session_store: FileAcpSessionStore::new(AcpFileSessionStoreOptions::new(state_dir)),
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
        prompt_queue_capacity,
    };
    std::sync::Arc::new(AcpRuntime::new(options))
}

fn env_with_delay(delay_ms: u64) -> SessionAgentOptions {
    SessionAgentOptions {
        env: Some(HashMap::from([(
            "ACP_FAKE_AGENT_PROMPT_DELAY_MS".to_string(),
            delay_ms.to_string(),
        )])),
        ..Default::default()
    }
}

fn turn_input(
    handle: boltz_acpx::runtime::public::AcpRuntimeHandle,
    request_id: &str,
) -> AcpRuntimeTurnInput {
    AcpRuntimeTurnInput {
        handle,
        text: "hello".to_string(),
        attachments: Vec::new(),
        mode: AcpRuntimePromptMode::Prompt,
        request_id: request_id.to_string(),
        timeout_ms: None,
    }
}

/// Success Criteria #1: two different sessions' `start_turn` calls both
/// begin executing without either waiting for the other. Session A gets a
/// slow (300ms) fake agent; session B is fast. Draining session B's result
/// well under session A's delay proves B never queued behind A — each
/// session spawns its own agent subprocess and has its own
/// `SessionPromptQueue`, so there is no shared lock between them.
#[test]
fn different_sessions_do_not_block_each_other() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path(), None);

        let handle_a = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "cross-session-a".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(env_with_delay(300)),
            })
            .await
            .expect("ensure_session A should succeed");
        let handle_b = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "cross-session-b".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect("ensure_session B should succeed");

        let turn_a = runtime.start_turn(turn_input(handle_a, "req-a")).await;
        let turn_b = runtime.start_turn(turn_input(handle_b, "req-b")).await;

        let start = Instant::now();
        let result_b = turn_b.result().await;
        let elapsed_b = start.elapsed();
        assert!(
            matches!(result_b, AcpRuntimeTurnResult::Completed { .. }),
            "expected session B to complete, got {result_b:?}"
        );
        assert!(
            elapsed_b < Duration::from_millis(250),
            "session B took {elapsed_b:?}, which suggests it waited on session A's slow turn"
        );

        // Drain A too so the test doesn't leave a dangling background task.
        let result_a = turn_a.result().await;
        assert!(matches!(result_a, AcpRuntimeTurnResult::Completed { .. }));
    });
}

/// Success Criteria #2: a second `start_turn` on the *same* session, issued
/// while the first is active, does not start until the first completes.
#[test]
fn same_session_second_turn_waits_for_first_to_finish() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path(), None);

        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "same-session-fifo".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(env_with_delay(300)),
            })
            .await
            .expect("ensure_session should succeed");

        // Admitted immediately (Idle -> Running); resolves without waiting
        // for the RPC itself to finish.
        let turn1 = runtime
            .start_turn(turn_input(handle.clone(), "req-1"))
            .await;

        // Submitted while turn1 is still running: must queue, and this
        // `.await` only resolves once turn1's slot frees.
        let before_second = Instant::now();
        let turn2 = runtime
            .start_turn(turn_input(handle.clone(), "req-2"))
            .await;
        let waited = before_second.elapsed();
        assert!(
            waited >= Duration::from_millis(250),
            "second same-session start_turn resolved after only {waited:?}; \
             expected it to wait for the first (300ms) turn to finish"
        );

        let result2 = turn2.result().await;
        assert!(matches!(result2, AcpRuntimeTurnResult::Completed { .. }));
        let result1 = turn1.result().await;
        assert!(matches!(result1, AcpRuntimeTurnResult::Completed { .. }));
    });
}

/// Success Criteria #3: exceeding the configured queue bound on one session
/// returns a specific, documented error/result variant (not a panic, not
/// silent dropping) and does not affect a concurrently-running different
/// session.
#[test]
fn exceeding_queue_capacity_reports_backpressure_without_affecting_other_sessions() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path(), Some(1));

        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "capacity-bound".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(env_with_delay(300)),
            })
            .await
            .expect("ensure_session should succeed");
        let other_handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "capacity-bound-other".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: None,
            })
            .await
            .expect("ensure_session for the other session should succeed");

        // Slot 0: runs immediately.
        let turn1 = runtime
            .start_turn(turn_input(handle.clone(), "req-1"))
            .await;
        // Pending slot 1/1 (capacity = 1): queues. Spawn it so it can be
        // polled (and thus actually occupy the pending slot) without
        // blocking this test on its ~300ms wait.
        let handle_for_task = handle.clone();
        let runtime_for_task = runtime.clone();
        let turn2_task = smol::spawn(async move {
            runtime_for_task
                .start_turn(turn_input(handle_for_task, "req-2"))
                .await
        });
        // Give the executor a chance to poll turn2's future at least once
        // so its (synchronous) admission actually runs before req-3 below.
        smol::Timer::after(Duration::from_millis(30)).await;
        assert_eq!(
            runtime.queue_len(&handle).unwrap(),
            1,
            "req-2 should have been admitted into the pending queue by now"
        );

        // Over capacity: rejected synchronously with a documented error,
        // not a panic and not a silently dropped request.
        let turn3 = runtime
            .start_turn(turn_input(handle.clone(), "req-3"))
            .await;
        let result3 = turn3.result().await;
        match result3 {
            AcpRuntimeTurnResult::Failed { error } => {
                assert_eq!(error.code.as_deref(), Some("ACP_TURN_QUEUE_FULL"));
            }
            other => panic!("expected a Failed/ACP_TURN_QUEUE_FULL result, got {other:?}"),
        }

        // The other session is untouched by session A's full queue.
        let other_turn = runtime
            .start_turn(turn_input(other_handle, "req-other"))
            .await;
        let other_result = other_turn.result().await;
        assert!(
            matches!(other_result, AcpRuntimeTurnResult::Completed { .. }),
            "a different session must not be affected by another session's full queue"
        );

        // Clean up: let the queued req-2 actually run to completion.
        let turn2 = turn2_task.await;
        let result2 = turn2.result().await;
        assert!(matches!(result2, AcpRuntimeTurnResult::Completed { .. }));
        let result1 = turn1.result().await;
        assert!(matches!(result1, AcpRuntimeTurnResult::Completed { .. }));
    });
}

/// Success Criteria #4: cancelling an active turn leaves queued-but-not-
/// started requests for that session intact (Step 6's documented default —
/// `cancel` alone does not clear the queue).
#[test]
fn cancel_active_leaves_queued_requests_intact() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path(), None);

        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "cancel-keeps-queue".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(env_with_delay(300)),
            })
            .await
            .expect("ensure_session should succeed");

        let turn1 = runtime
            .start_turn(turn_input(handle.clone(), "req-1"))
            .await;

        let handle_for_task = handle.clone();
        let runtime_for_task = runtime.clone();
        let turn2_task = smol::spawn(async move {
            runtime_for_task
                .start_turn(turn_input(handle_for_task, "req-2"))
                .await
        });
        smol::Timer::after(Duration::from_millis(30)).await;
        assert_eq!(
            runtime.queue_len(&handle).unwrap(),
            1,
            "req-2 should be queued behind the active req-1"
        );

        runtime
            .cancel(&handle, Some("test cancel"))
            .await
            .expect("cancel should succeed while a prompt is active");

        assert_eq!(
            runtime.queue_len(&handle).unwrap(),
            1,
            "cancelling the active turn must not clear queued-but-not-started requests"
        );

        // req-2 still runs to completion once req-1's slot frees.
        let turn2 = turn2_task.await;
        let result2 = turn2.result().await;
        assert!(matches!(result2, AcpRuntimeTurnResult::Completed { .. }));

        // req-1's own terminal state doesn't matter for this criterion
        // (the fake agent isn't a true cooperative-cancellation agent); just
        // drain it so no task is left dangling.
        let _ = turn1.result().await;
    });
}

/// `cancel_active_and_clear` is the explicit "stop everything" opt-in: it
/// does drop queued requests, unlike plain `cancel`.
#[test]
fn cancel_active_and_clear_drops_queued_requests() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path(), None);

        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "cancel-and-clear".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(env_with_delay(300)),
            })
            .await
            .expect("ensure_session should succeed");

        let turn1 = runtime
            .start_turn(turn_input(handle.clone(), "req-1"))
            .await;
        let handle_for_task = handle.clone();
        let runtime_for_task = runtime.clone();
        let turn2_task = smol::spawn(async move {
            runtime_for_task
                .start_turn(turn_input(handle_for_task, "req-2"))
                .await
        });
        smol::Timer::after(Duration::from_millis(30)).await;
        assert_eq!(runtime.queue_len(&handle).unwrap(), 1);

        let cleared = runtime
            .cancel_active_and_clear(&handle, Some("stop everything"))
            .await
            .expect("cancel_active_and_clear should succeed");
        assert_eq!(cleared, 1, "expected the one queued request to be cleared");
        assert_eq!(runtime.queue_len(&handle).unwrap(), 0);

        // req-2's future now resolves with a Cleared-derived Failed result
        // rather than ever actually running.
        let turn2 = turn2_task.await;
        let result2 = turn2.result().await;
        assert!(
            matches!(result2, AcpRuntimeTurnResult::Failed { .. }),
            "a cleared queued request must resolve to a Failed turn, not hang forever"
        );

        let _ = turn1.result().await;
    });
}

/// Success Criteria (Requirement 4 / Step 9e): `session/update` notification
/// processing for one session is applied in the order the agent sent them.
#[test]
fn session_updates_are_applied_in_arrival_order() {
    smol::block_on(async {
        let state_dir = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let runtime = test_runtime(state_dir.path(), cwd.path(), None);

        let session_options = SessionAgentOptions {
            env: Some(HashMap::from([(
                "ACP_FAKE_AGENT_PROMPT_UPDATE_COUNT".to_string(),
                "5".to_string(),
            )])),
            ..Default::default()
        };
        let handle = runtime
            .ensure_session(AcpRuntimeEnsureInput {
                session_key: "update-ordering".to_string(),
                agent: "test-agent".to_string(),
                mode: AcpRuntimeSessionMode::Persistent,
                resume_session_id: None,
                cwd: None,
                session_options: Some(session_options),
            })
            .await
            .expect("ensure_session should succeed");

        let mut turn = runtime.start_turn(turn_input(handle, "req-ordering")).await;
        let mut events = turn.events();
        let mut chunks = Vec::new();
        while let Some(event) = events.next().await {
            if let AcpRuntimeEvent::TextDelta { text, .. } = event {
                chunks.push(text);
            }
        }

        assert_eq!(
            chunks,
            vec!["chunk-0", "chunk-1", "chunk-2", "chunk-3", "chunk-4"],
            "session/update notifications must be applied in the order the agent sent them"
        );

        let result = turn.result().await;
        assert!(matches!(result, AcpRuntimeTurnResult::Completed { .. }));
    });
}
