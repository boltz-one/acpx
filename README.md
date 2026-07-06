# acpx

**Agent Client Protocol (ACP) client + session runtime, in Rust.**

`boltz-acpx` is an embeddable, [`smol`](https://docs.rs/smol)-based library for
driving [ACP](https://agentclientprotocol.com) coding agents — Claude
(`claude-agent-acp`), Codex, Gemini, Copilot, Cursor, and any other
ACP-speaking backend. It owns the whole client side of the protocol: the
`initialize` handshake, session lifecycle with transparent reconnect,
per-session prompt queueing, a permission policy engine, filesystem/terminal
client tools, and JSON session persistence.

It is a Rust port of the *core client + session-persistence + in-process
queueing* surface of the `acpx` TypeScript CLI. The CLI/commander layer, the
cross-process IPC queue daemon, and the flows DSL are intentionally **out of
scope** — this is a library, not a CLI.

## Features

- **Handshake & transport** — ndjson-over-stdio JSON-RPC 2.0 via the official
  [`agent-client-protocol`](https://crates.io/crates/agent-client-protocol) SDK,
  including per-agent quirks (Gemini `--acp`/`--experimental-acp` version
  rewrite, Copilot capability probe, Devin/Windsurf client-identity, Claude
  `session/new` timeout, Windows `.cmd`/`.bat` shell wrapping).
- **Session lifecycle** — `session/new` · `resume` · `load` · `close`, with a
  reconnect state machine that transparently re-acquires a backend session
  (resume → load → fresh fallback) after an agent crash and replays the saved
  mode/model/config-option preferences.
- **Prompt queueing** — a bounded, per-session FIFO so overlapping prompts on
  one session run in order while different sessions run concurrently.
- **Permissions** — a programmatic `PermissionPolicy` (auto-approve / auto-deny
  / escalate rules), an async non-blocking permission handler, an escalation
  audit callback, and permission-decision stats.
- **Client tools** — sandboxed filesystem read/write and a terminal manager
  (create/output/wait/kill/release) with process-group tracking, surfaced as
  live `ClientOperation` progress events.
- **Persistence** — versioned, atomic JSON session records (conversation,
  model state, config options) with import/export and prune.

## Quick start

```rust,ignore
use boltz_acpx::runtime::public::{
    AcpRuntime, AcpRuntimeOptions, AcpRuntimeEnsureInput, AcpRuntimeTurnInput,
    AcpRuntimeSessionMode, AcpRuntimePromptMode, BuiltInAgentRegistry,
};
use boltz_acpx::session::persistence::FileAcpSessionStore;
use boltz_acpx::session::store_options::AcpFileSessionStoreOptions;
use boltz_acpx::types::{PermissionMode, NonInteractivePermissionPolicy};
use futures::StreamExt;

smol::block_on(async {
    let runtime = AcpRuntime::new(AcpRuntimeOptions {
        cwd: std::env::current_dir().unwrap(),
        session_store: FileAcpSessionStore::new(
            AcpFileSessionStoreOptions::new("/tmp/acpx-sessions"),
        ),
        agent_registry: std::sync::Arc::new(BuiltInAgentRegistry::new(None)),
        permission_mode: PermissionMode::ApproveAll,
        non_interactive_permissions: NonInteractivePermissionPolicy::Deny,
        ..Default::default() // if you add a Default; otherwise fill remaining fields
    });

    let handle = runtime.ensure_session(AcpRuntimeEnsureInput {
        session_key: "demo".into(),
        agent: "claude".into(), // resolves to npx @agentclientprotocol/claude-agent-acp
        mode: AcpRuntimeSessionMode::Persistent,
        resume_session_id: None,
        cwd: None,
        session_options: None,
    }).await?;

    let mut turn = runtime.start_turn(AcpRuntimeTurnInput {
        handle,
        text: "Reply with exactly one word: pong".into(),
        attachments: Vec::new(),
        mode: AcpRuntimePromptMode::Prompt,
        request_id: "req-1".into(),
        timeout_ms: None,
    }).await;

    let mut events = turn.events();
    while let Some(event) = events.next().await { /* TextDelta, ToolCall, ClientOperation, … */ }
    let _ = turn.result().await;
});
```

Built-in agent names resolve to their launch commands via `BuiltInAgentRegistry`
(e.g. `claude` → `npx -y @agentclientprotocol/claude-agent-acp@^0.37.0`); pass a
raw command line or an override map for anything else.

## Testing

The default test suite is **hermetic** — it drives an in-tree fake ACP agent
(`tests/fixtures/fake_agent`), no network or real model required:

```sh
cargo test --features test-support
```

A non-hermetic **real-agent smoke test** (gated `#[ignore]`) drives the actual
`@agentclientprotocol/claude-agent-acp` adapter and needs `npx` + a working
Claude Code auth on the host:

```sh
cargo test --features test-support --test real_agent_smoke -- --ignored --nocapture
```

## Async runtime & platform

`smol` throughout (no `tokio`). Subprocess spawn/kill uses vendored,
session-group-aware primitives (`src/util/`). Unix is the primary target;
Windows is supported (batch-shell agent spawn, `TerminateProcess` liveness) but
lightly CI-tested.

## Architecture decisions

Design rationale — SDK reuse, `smol` substrate, the per-session prompt queue,
the session-persistence format, the async permission API, and the long-lived
client-per-session model — is recorded in [`docs/decisions/`](docs/decisions/).

## License

Apache-2.0. See [LICENSE](LICENSE).
