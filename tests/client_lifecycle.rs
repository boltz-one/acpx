#![cfg(feature = "test-support")]
//! Phase 2 integration tests: spawn the real fake ACP agent binary
//! (`tests/fixtures/fake_agent/main.rs`) as a subprocess and drive the
//! handshake/shutdown lifecycle end to end over real ndjson stdio. No
//! mocks — run with `cargo test -p boltz-acpx --features test-support`.

use std::collections::HashMap;
use std::path::Path;

use boltz_acpx::client::{AcpClient, SpawnAgentOptions};
use boltz_acpx::error::AcpError;

fn fake_agent_path() -> &'static str {
    env!("CARGO_BIN_EXE_acp-fake-agent")
}

#[test]
fn spawn_handshake_and_shutdown() {
    smol::block_on(async {
        let env = HashMap::new();
        let client = AcpClient::spawn(SpawnAgentOptions {
            program: fake_agent_path(),
            args: &[],
            cwd: Path::new("/tmp"),
            env: &env,
            client_name: "boltz-acpx-test".to_string(),
            terminal: true,
            is_gemini: false,
            is_devin: false,
            handlers: Default::default(),
            auth_credentials: None,
        })
        .await
        .expect("spawn+handshake should succeed");

        assert!(!client.init_response().agent_capabilities.load_session);

        let info = client.shutdown().await;
        assert_eq!(info.reason, "sigterm");
    });
}

#[test]
fn sigterm_ignoring_agent_escalates_to_sigkill() {
    smol::block_on(async {
        let mut env = HashMap::new();
        env.insert("ACP_FAKE_AGENT_IGNORE_SIGTERM".to_string(), "1".to_string());
        let client = AcpClient::spawn(SpawnAgentOptions {
            program: fake_agent_path(),
            args: &[],
            cwd: Path::new("/tmp"),
            env: &env,
            client_name: "boltz-acpx-test".to_string(),
            terminal: false,
            is_gemini: false,
            is_devin: false,
            handlers: Default::default(),
            auth_credentials: None,
        })
        .await
        .expect("spawn+handshake should succeed");

        let info = client.shutdown().await;
        assert_eq!(info.reason, "sigkill");
    });
}

#[test]
fn session_new_returns_typed_response() {
    smol::block_on(async {
        let env = HashMap::new();
        let client = AcpClient::spawn(SpawnAgentOptions {
            program: fake_agent_path(),
            args: &[],
            cwd: Path::new("/tmp"),
            env: &env,
            client_name: "boltz-acpx-test".to_string(),
            terminal: false,
            is_gemini: false,
            is_devin: false,
            handlers: Default::default(),
            auth_credentials: None,
        })
        .await
        .expect("spawn+handshake should succeed");

        let response = client
            .session_new(Path::new("/tmp").to_path_buf(), vec![])
            .await
            .expect("session/new should succeed");
        // Phase 4 changed the fake agent to suffix the session id with its
        // own pid (see `tests/fixtures/fake_agent/main.rs`'s module docs),
        // so a reconnect-after-crash test can tell resumed sessions (id
        // unchanged) apart from freshly created ones (new pid suffix).
        assert!(response.session_id.0.starts_with("fake-session-"));

        client.shutdown().await;
    });
}

#[test]
fn devin_spawn_advertises_windsurf_client_identity() {
    // Ports the deferred Devin runtime quirk: `SpawnAgentOptions.is_devin`
    // must swap the advertised `clientInfo`/`clientCapabilities` for
    // Devin's Windsurf compatibility identity during the real `initialize`
    // handshake, not just in a unit-tested helper function. The fake agent
    // echoes what it received back under `_meta` (see
    // `tests/fixtures/fake_agent/main.rs`) so this test can assert on the
    // wire-level request, not just this crate's internal call.
    smol::block_on(async {
        let env = HashMap::new();
        let client = AcpClient::spawn(SpawnAgentOptions {
            program: fake_agent_path(),
            args: &[],
            cwd: Path::new("/tmp"),
            env: &env,
            client_name: "boltz-acpx-test".to_string(),
            terminal: false,
            is_gemini: false,
            is_devin: true,
            handlers: Default::default(),
            auth_credentials: None,
        })
        .await
        .expect("spawn+handshake should succeed");

        let meta = client
            .init_response()
            .meta
            .as_ref()
            .expect("fake agent should echo request info back under _meta");

        let echoed_client_info = meta
            .get("echoClientInfo")
            .expect("echoClientInfo should be present");
        assert_eq!(echoed_client_info["name"], "windsurf");
        assert_eq!(echoed_client_info["version"], "1.110.1");

        let echoed_capabilities = meta
            .get("echoClientCapabilities")
            .expect("echoClientCapabilities should be present");
        assert_eq!(
            echoed_capabilities["_meta"]["cognition.ai/requestDiagnostics"],
            true
        );

        client.shutdown().await;
    });
}

#[test]
fn gemini_startup_timeout_kills_hung_agent_and_reports_diagnostic() {
    // Ports the deferred Gemini runtime quirk: when `is_gemini` is set,
    // `AcpClient::spawn` must race the `initialize` handshake against
    // `resolve_gemini_acp_startup_timeout_ms()` instead of waiting
    // indefinitely, kill the hung subprocess, and surface
    // `AcpError::GeminiAcpStartupTimeout` with a diagnostic message. Uses
    // the real env-var override (matching production's only configuration
    // knob for this timeout) plus the fake agent's initialize-delay toggle
    // to force a real timeout quickly instead of waiting 15s.
    //
    // Safety: `set_var` only affects this process's env, and no other test
    // in this binary reads `ACPX_GEMINI_ACP_STARTUP_TIMEOUT_MS`.
    unsafe {
        std::env::set_var("ACPX_GEMINI_ACP_STARTUP_TIMEOUT_MS", "100");
    }

    smol::block_on(async {
        let mut env = HashMap::new();
        env.insert(
            "ACP_FAKE_AGENT_INITIALIZE_DELAY_MS".to_string(),
            "5000".to_string(),
        );
        let result = AcpClient::spawn(SpawnAgentOptions {
            program: fake_agent_path(),
            args: &[],
            cwd: Path::new("/tmp"),
            env: &env,
            client_name: "boltz-acpx-test".to_string(),
            terminal: false,
            is_gemini: true,
            is_devin: false,
            handlers: Default::default(),
            auth_credentials: None,
        })
        .await;

        match result {
            Ok(_) => panic!("expected initialize to time out, but it succeeded"),
            Err(AcpError::GeminiAcpStartupTimeout(message)) => {
                assert!(message.contains("startup timed out"));
            }
            Err(other) => panic!("expected GeminiAcpStartupTimeout, got {other:?}"),
        }
    });

    unsafe {
        std::env::remove_var("ACPX_GEMINI_ACP_STARTUP_TIMEOUT_MS");
    }
}

#[test]
fn non_devin_spawn_advertises_real_client_identity() {
    smol::block_on(async {
        let env = HashMap::new();
        let client = AcpClient::spawn(SpawnAgentOptions {
            program: fake_agent_path(),
            args: &[],
            cwd: Path::new("/tmp"),
            env: &env,
            client_name: "boltz-acpx-test".to_string(),
            terminal: false,
            is_gemini: false,
            is_devin: false,
            handlers: Default::default(),
            auth_credentials: None,
        })
        .await
        .expect("spawn+handshake should succeed");

        let meta = client
            .init_response()
            .meta
            .as_ref()
            .expect("fake agent should echo request info back under _meta");
        let echoed_client_info = meta
            .get("echoClientInfo")
            .expect("echoClientInfo should be present");
        assert_eq!(echoed_client_info["name"], "boltz-acpx-test");
        assert!(
            meta.get("echoClientCapabilities")
                .unwrap()
                .get("_meta")
                .is_none()
        );

        client.shutdown().await;
    });
}

#[test]
fn authenticate_selects_advertised_method_when_credential_present() {
    // Gap 3: the agent advertises an auth method at `initialize`, and the
    // app supplies a matching credential — `AcpClient::spawn` must send the
    // `authenticate` RPC selecting that method before returning. The fake
    // agent records the received `methodId` and echoes it back under
    // `session/new`'s `_meta.authenticatedMethod`, so this asserts on the
    // real wire exchange, not just this crate's internal call.
    smol::block_on(async {
        let mut env = HashMap::new();
        env.insert(
            "ACP_FAKE_AGENT_AUTH_METHOD".to_string(),
            "oauth".to_string(),
        );
        let creds = HashMap::from([("oauth".to_string(), "token-123".to_string())]);
        let client = AcpClient::spawn(SpawnAgentOptions {
            program: fake_agent_path(),
            args: &[],
            cwd: Path::new("/tmp"),
            env: &env,
            client_name: "boltz-acpx-test".to_string(),
            terminal: false,
            is_gemini: false,
            is_devin: false,
            handlers: Default::default(),
            auth_credentials: Some(creds),
        })
        .await
        .expect("spawn+handshake+authenticate should succeed");

        let response = client
            .session_new(Path::new("/tmp").to_path_buf(), vec![])
            .await
            .expect("session/new should succeed");
        let authenticated = response
            .meta
            .as_ref()
            .and_then(|m| m.get("authenticatedMethod"))
            .and_then(|v| v.as_str());
        assert_eq!(
            authenticated,
            Some("oauth"),
            "client should have sent authenticate with the advertised method id"
        );

        client.shutdown().await;
    });
}

#[test]
fn authenticate_skipped_when_no_credential_resolves() {
    // Gap 3 (Requirement 4 / plan Unresolved Questions #6 default): the agent
    // advertises an auth method but no credential resolves (no app map, no
    // ambient `ACP_AUTH_*`) — the client must proceed WITHOUT sending
    // `authenticate`, letting the agent reject a later RPC if it truly
    // requires auth, rather than hanging or failing the handshake.
    smol::block_on(async {
        let mut env = HashMap::new();
        env.insert(
            "ACP_FAKE_AGENT_AUTH_METHOD".to_string(),
            "oauth".to_string(),
        );
        let client = AcpClient::spawn(SpawnAgentOptions {
            program: fake_agent_path(),
            args: &[],
            cwd: Path::new("/tmp"),
            env: &env,
            client_name: "boltz-acpx-test".to_string(),
            terminal: false,
            is_gemini: false,
            is_devin: false,
            handlers: Default::default(),
            auth_credentials: None,
        })
        .await
        .expect("spawn should still succeed without authenticating");

        let response = client
            .session_new(Path::new("/tmp").to_path_buf(), vec![])
            .await
            .expect("session/new should succeed");
        let authenticated = response
            .meta
            .as_ref()
            .and_then(|m| m.get("authenticatedMethod"))
            .and_then(|v| v.as_str());
        assert_eq!(
            authenticated, None,
            "no credential -> authenticate must not be sent"
        );

        client.shutdown().await;
    });
}
