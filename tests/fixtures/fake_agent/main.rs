//! Minimal real ACP agent for Phase 2-4 integration tests. Speaks JSON-RPC
//! 2.0 over ndjson stdio (matching acpx's `test/mock-agent.ts` role, scoped
//! down to what these phases' tests exercise: `initialize`, `session/new`,
//! `session/prompt` (with a live `session/update` notification), and
//! `session/resume` (Phase 4's reconnect-after-crash test).
//!
//! Behavior toggles via env var (kept as one process, not a CLI flag parser,
//! since these are test-only knobs):
//! - `ACP_FAKE_AGENT_IGNORE_SIGTERM=1`: installs a `SIG_IGN` handler for
//!   SIGTERM before serving, so the SIGTERM->SIGKILL escalation test can
//!   observe the escalation actually happening.
//! - `ACP_FAKE_AGENT_INITIALIZE_DELAY_MS=<n>`: sleeps `n` milliseconds on
//!   the request-serving thread before responding to `initialize`. Used by
//!   `client_lifecycle.rs`'s Gemini-startup-timeout test to force a real
//!   `AcpClient::spawn` call past `resolve_gemini_acp_startup_timeout_ms`'s
//!   budget without an actual slow Gemini CLI installed.
//! - `ACP_FAKE_AGENT_EXIT_AFTER_MS=<n>`: exits the process (simulating a
//!   crash, no graceful shutdown) `n` milliseconds after startup. Set via
//!   `AcpRuntimeEnsureInput.session_options.env` (persisted onto the
//!   session record) so Phase 4's reconnect-after-crash integration test
//!   can make a *specific* spawned agent die without the test needing raw
//!   pid access to the process the runtime spawned internally.
//! - `ACP_FAKE_AGENT_PROMPT_DELAY_MS=<n>`: sleeps `n` milliseconds on the
//!   request-serving thread before responding to `session/prompt` (the
//!   live `session/update` notification is still emitted immediately, so a
//!   test can observe "a turn is in flight" before the delay elapses).
//!   Phase 6's queueing tests use this, same env-var-toggle pattern as
//!   above, to (a) prove two *different* sessions' turns overlap in time
//!   (each session gets its own spawned agent process, so one slow agent
//!   does not block another), and (b) prove a second `start_turn` on the
//!   *same* session does not begin until the first's delay has elapsed.
//! - `ACP_FAKE_AGENT_PROMPT_UPDATE_COUNT=<n>`: emits `n` sequential
//!   `session/update` notifications (`chunk-0`, `chunk-1`, ...) instead of
//!   the default single `"hello from fake agent"` chunk. Defaults to `1`
//!   (unset), preserving every existing test's assumption of exactly one
//!   text delta. Phase 6's notification-ordering test sets this to observe
//!   several updates and assert they are processed in the order sent.
//! - `session/new` returns a session id suffixed with this process's pid
//!   (`fake-session-<pid>`), so a test can tell whether a second
//!   `ensure_session` call resumed the original backend session
//!   (`session/resume`, id unchanged) or silently created a new one
//!   (`session/new` again, id would change to the new process's pid).
//!
//! Phase 4 additions (gaps 9/11/12/15):
//! - `ACP_FAKE_AGENT_ADVERTISE_CLOSE=1`: adds `sessionCapabilities.close: {}`
//!   to the `initialize` response, independent of the resume/load profile
//!   selected via `ACP_FAKE_AGENT_SESSION_CAPABILITIES`.
//! - `ACP_FAKE_AGENT_RPC_LOG=<path>`: appends the method name of every
//!   handled request/notification to `<path>`, one per line — lets a test
//!   assert an RPC was (or was not) actually sent, without needing to
//!   inspect the transport directly.
//! - `ACP_FAKE_AGENT_SET_MODE_ERROR_CODE=<code>`: like the existing
//!   resume/load/set_config_option error-injection knobs, but for
//!   `session/set_mode` (gap 11's rejected-mode test).
//! - `ACP_FAKE_AGENT_MODEL_CONFIG=1`: `session/new`/`session/load` include a
//!   `model`-category `configOptions` select entry
//!   (current value from `ACP_FAKE_AGENT_MODEL_CURRENT`, default
//!   `"default-model"`; options `default-model`/`gpt-5`); `session/set_config_option`
//!   echoes back `configOptions` with `currentValue` set to whatever value
//!   the request asked for (so a live model-application call observes the
//!   agent's designated model, gap 12's test).
//! - `ACP_FAKE_AGENT_OMIT_CONFIG_AFTER_FIRST_FILE=<path>`: on the first
//!   `session/new` call across any process sharing this path (tracked via a
//!   marker file, since gap 15's test needs two *different* fake-agent
//!   processes — one per fresh-session connection attempt), include the
//!   `ACP_FAKE_AGENT_MODEL_CONFIG` `configOptions` shape; every call after
//!   the marker file exists omits `configOptions` entirely, simulating a
//!   later reconnect whose fresh session no longer advertises it.
//!
//! The request-serving loop runs on a background thread; the main thread
//! parks indefinitely instead of returning when stdin reaches EOF. A real
//! ACP agent's process lifetime is controlled by signals (this is exactly
//! what `client/shutdown.rs` exercises), not by "my stdin pipe closed" — the
//! shutdown sequence closes stdin *before* sending SIGTERM (mirroring
//! acpx's `client-process.ts`), so a process that exits on stdin-EOF would
//! make the SIGTERM/SIGKILL escalation untestable. Phase 4's crash-recovery
//! test instead sends SIGKILL directly to the recorded pid, simulating an
//! unclean death the shutdown sequence never runs for.

use std::io::{BufRead, Write};
use std::sync::Mutex;

use serde_json::{Value, json};

/// Records the `methodId` of the last `authenticate` request received, so
/// `session/new` can echo it back under `_meta.authenticatedMethod` and a
/// test can assert the client actually sent `authenticate` with the expected
/// method (gap 3). `Mutex::new` is const, so this needs no lazy init.
static LAST_AUTHENTICATE_METHOD: Mutex<Option<String>> = Mutex::new(None);

fn main() {
    #[cfg(unix)]
    if std::env::var("ACP_FAKE_AGENT_IGNORE_SIGTERM").as_deref() == Ok("1") {
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
    }

    if let Ok(ms) = std::env::var("ACP_FAKE_AGENT_EXIT_AFTER_MS") {
        if let Ok(ms) = ms.parse::<u64>() {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                std::process::exit(1);
            });
        }
    }

    std::thread::spawn(serve_stdio);

    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

fn serve_stdio() {
    let stdin = std::io::stdin();
    let stdout = Mutex::new(std::io::stdout());

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        handle_message(&request, &stdout);
    }
}

fn write_line(stdout: &Mutex<std::io::Stdout>, payload: &Value) {
    let mut stdout = stdout.lock().expect("stdout mutex poisoned");
    let _ = writeln!(stdout, "{payload}");
    let _ = stdout.flush();
}

/// Gap 9/11/12 test support: appends `method` to the file named by
/// `ACP_FAKE_AGENT_RPC_LOG`, if set. Best-effort (a missing/unwritable path
/// is silently ignored — this is test plumbing, not agent behavior under
/// test).
fn log_rpc_method(method: &str) {
    let Ok(path) = std::env::var("ACP_FAKE_AGENT_RPC_LOG") else {
        return;
    };
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{method}");
    }
}

/// Gap 15 test support: returns `true` the FIRST time it's called for a
/// given `path` across any process (tracked via the marker file's
/// existence, since two different fake-agent processes — one per
/// reconnect's fresh `session/new` — need to agree on "have we already
/// advertised config options once").
fn is_first_call_for_marker(path: &str) -> bool {
    if std::path::Path::new(path).exists() {
        return false;
    }
    let _ = std::fs::write(path, b"1");
    true
}

/// Ports the `ACP_FAKE_AGENT_MODEL_CONFIG` shape: a `model`-category
/// `configOptions` select entry with `current` as its current value.
fn model_config_options(current: &str) -> Value {
    json!([{
        "id": "model",
        "name": "Model",
        "category": "model",
        "type": "select",
        "currentValue": current,
        "options": [
            {"value": "default-model", "name": "default-model"},
            {"value": "gpt-5", "name": "gpt-5"},
        ],
    }])
}

/// Handles one JSON-RPC message (request or notification). Requests with an
/// `id` always get exactly one response line; `session/prompt` additionally
/// emits a `session/update` notification first, so integration tests have a
/// live event to drain before the terminal response arrives.
fn handle_message(request: &Value, stdout: &Mutex<std::io::Stdout>) {
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return;
    };
    log_rpc_method(method);
    let id = request.get("id").cloned();

    if method == "initialize" {
        if let Ok(ms) = std::env::var("ACP_FAKE_AGENT_INITIALIZE_DELAY_MS") {
            if let Ok(ms) = ms.parse::<u64>() {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
        }
    }

    if method == "session/prompt" {
        let session_id = request
            .get("params")
            .and_then(|p| p.get("sessionId"))
            .cloned()
            .unwrap_or(json!("fake-session-1"));
        // Gap 1/2/25: when set, issue an agent->client
        // `session/request_permission` request with the given tool title, so
        // an integration test can drive the client's permission decision
        // tree (policy match -> escalation callback + stats) over the real
        // handshake-registered handler. Fire-and-forget: the client's
        // response arrives back on stdin and is ignored (no `method`).
        if let Ok(title) = std::env::var("ACP_FAKE_AGENT_REQUEST_PERMISSION") {
            write_line(
                stdout,
                &json!({
                    "jsonrpc": "2.0",
                    "id": "perm-1",
                    "method": "session/request_permission",
                    "params": {
                        "sessionId": session_id,
                        "toolCall": {"toolCallId": "tool-1", "title": title},
                        "options": [
                            {"optionId": "allow_once", "name": "Allow", "kind": "allow_once"},
                            {"optionId": "reject_once", "name": "Reject", "kind": "reject_once"},
                        ],
                    },
                }),
            );
        }
        // Gap 20: when set, issue an agent->client `fs/read_text_file`
        // request for the given path, so an integration test can drive the
        // client's filesystem handler and observe the resulting
        // `client_operation` event streamed by the turn. Fire-and-forget
        // (the client's response arrives on stdin and is ignored).
        if let Ok(path) = std::env::var("ACP_FAKE_AGENT_READ_FILE") {
            write_line(
                stdout,
                &json!({
                    "jsonrpc": "2.0",
                    "id": "fs-1",
                    "method": "fs/read_text_file",
                    "params": {"sessionId": session_id, "path": path},
                }),
            );
        }
        // Unset defaults to 1 (every existing test's assumption); an explicit
        // `0` sends NO update — gap 6's regression test needs a genuine
        // no-reply timeout, so `.max(1)` must not floor an explicit 0.
        let update_count: u32 = std::env::var("ACP_FAKE_AGENT_PROMPT_UPDATE_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        for i in 0..update_count {
            let text = if update_count <= 1 {
                "hello from fake agent".to_string()
            } else {
                format!("chunk-{i}")
            };
            write_line(
                stdout,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": session_id,
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": {"type": "text", "text": text},
                        },
                    },
                }),
            );
        }
        if let Ok(ms) = std::env::var("ACP_FAKE_AGENT_PROMPT_DELAY_MS") {
            if let Ok(ms) = ms.parse::<u64>() {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
        }
    }

    let Some(id) = id else {
        // A notification (e.g. `session/cancel`) — nothing to respond to.
        return;
    };

    // Gap 5: optional per-method response delay (for timeout-path tests) and
    // JSON-RPC error injection (for resume/load fallback + replay-rollback
    // tests). Both are keyed by the method being handled.
    let delay_env = match method {
        "session/new" => "ACP_FAKE_AGENT_SESSION_NEW_DELAY_MS",
        "session/resume" => "ACP_FAKE_AGENT_RESUME_DELAY_MS",
        "session/load" => "ACP_FAKE_AGENT_LOAD_DELAY_MS",
        _ => "",
    };
    if !delay_env.is_empty() {
        if let Ok(ms) = std::env::var(delay_env) {
            if let Ok(ms) = ms.parse::<u64>() {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
        }
    }
    let error_env = match method {
        "session/resume" => "ACP_FAKE_AGENT_RESUME_ERROR_CODE",
        "session/load" => "ACP_FAKE_AGENT_LOAD_ERROR_CODE",
        "session/set_config_option" => "ACP_FAKE_AGENT_SET_CONFIG_ERROR_CODE",
        // Gap 11: rejected `session/set_mode` test support.
        "session/set_mode" => "ACP_FAKE_AGENT_SET_MODE_ERROR_CODE",
        _ => "",
    };
    if !error_env.is_empty() {
        if let Ok(code) = std::env::var(error_env) {
            if let Ok(code) = code.parse::<i64>() {
                write_line(
                    stdout,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": code, "message": format!("injected error for {method}")},
                    }),
                );
                return;
            }
        }
    }

    let result = match method {
        // Echoes the request's `clientInfo`/`clientCapabilities` back
        // under `_meta` (a reserved, implementation-defined field per ACP)
        // so integration tests can assert what this crate actually
        // advertised — used by `client_lifecycle.rs`'s Devin
        // identity-spoofing test.
        "initialize" => {
            let params = request.get("params").cloned().unwrap_or(json!({}));
            // Gap 3: when set, advertise a single agent-type auth method so a
            // test can exercise the client's post-initialize `authenticate`
            // selection. An untyped `{id, name}` entry deserializes as the
            // default `Agent` auth-method variant SDK-side.
            let auth_methods = match std::env::var("ACP_FAKE_AGENT_AUTH_METHOD") {
                Ok(id) if !id.is_empty() => json!([{"id": id, "name": "Test Auth"}]),
                _ => json!([]),
            };
            // Gap 5: select which session-acquisition capabilities to
            // advertise, so tests can exercise the Resume / Load /
            // RequireSameSession branches. Default (unset or "resume")
            // preserves the prior hardcoded resumable behavior every existing
            // test relies on.
            let mut agent_capabilities = match std::env::var("ACP_FAKE_AGENT_SESSION_CAPABILITIES")
                .unwrap_or_default()
                .as_str()
            {
                "load" => json!({"loadSession": true, "sessionCapabilities": {}}),
                "none" => json!({"loadSession": false, "sessionCapabilities": {}}),
                _ => json!({"loadSession": false, "sessionCapabilities": {"resume": {}}}),
            };
            // Gap 9: independent of the resume/load profile above, so tests
            // can exercise close-capability-gating orthogonally.
            if std::env::var("ACP_FAKE_AGENT_ADVERTISE_CLOSE").as_deref() == Ok("1") {
                agent_capabilities["sessionCapabilities"]["close"] = json!({});
            }
            json!({
                "protocolVersion": 1,
                "agentCapabilities": agent_capabilities,
                "authMethods": auth_methods,
                "_meta": {
                    "echoClientInfo": params.get("clientInfo").cloned().unwrap_or(Value::Null),
                    "echoClientCapabilities": params
                        .get("clientCapabilities")
                        .cloned()
                        .unwrap_or(Value::Null),
                },
            })
        }
        "session/new" => {
            let mut response = json!({
                "sessionId": format!("fake-session-{}", std::process::id()),
                // Gap 3: echo the auth method the client selected (if any) so
                // the integration test can assert `authenticate` was actually
                // sent with the expected id. `null` when no `authenticate`
                // ran.
                "_meta": {
                    "authenticatedMethod": LAST_AUTHENTICATE_METHOD
                        .lock()
                        .expect("auth mutex poisoned")
                        .clone(),
                },
            });
            // Gap 15: the marker-file toggle takes precedence over the plain
            // `ACP_FAKE_AGENT_MODEL_CONFIG` toggle when both are set, so a
            // test can drive "first connection advertises, second omits"
            // across two different fake-agent processes sharing one marker
            // path.
            let model_current = std::env::var("ACP_FAKE_AGENT_MODEL_CURRENT")
                .unwrap_or_else(|_| "default-model".to_string());
            let advertise_config =
                if let Ok(marker) = std::env::var("ACP_FAKE_AGENT_OMIT_CONFIG_AFTER_FIRST_FILE") {
                    is_first_call_for_marker(&marker)
                } else {
                    std::env::var("ACP_FAKE_AGENT_MODEL_CONFIG").as_deref() == Ok("1")
                };
            if advertise_config {
                response["configOptions"] = model_config_options(&model_current);
            }
            response
        }
        "session/resume" => json!({}),
        // Gap 5: real `session/load` handler (previously fell into the
        // catch-all), so the Load acquisition path can run end to end.
        "session/load" => {
            let mut response = json!({});
            if std::env::var("ACP_FAKE_AGENT_MODEL_CONFIG").as_deref() == Ok("1") {
                let model_current = std::env::var("ACP_FAKE_AGENT_MODEL_CURRENT")
                    .unwrap_or_else(|_| "default-model".to_string());
                response["configOptions"] = model_config_options(&model_current);
            }
            response
        }
        "session/prompt" => json!({
            "stopReason": "end_turn",
        }),
        "session/set_mode" => json!({}),
        "session/set_config_option" => {
            let requested_value = request
                .get("params")
                .and_then(|p| p.get("value"))
                .and_then(Value::as_str)
                .unwrap_or("default-model")
                .to_string();
            json!({ "configOptions": model_config_options(&requested_value) })
        }
        // Gap 9: real `session/close` handler.
        "session/close" => json!({}),
        // Gap 3: record which auth method the client selected.
        "authenticate" => {
            if let Some(method_id) = request
                .get("params")
                .and_then(|p| p.get("methodId"))
                .and_then(Value::as_str)
            {
                *LAST_AUTHENTICATE_METHOD
                    .lock()
                    .expect("auth mutex poisoned") = Some(method_id.to_string());
            }
            json!({})
        }
        _ => json!({}),
    };

    write_line(
        stdout,
        &json!({"jsonrpc": "2.0", "id": id, "result": result}),
    );
}
