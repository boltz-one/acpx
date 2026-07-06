# 0001. Reuse the `agent-client-protocol` SDK instead of hand-rolling ACP

- **Status:** accepted
- **Date:** 2026-07-05
- **Lane:** high-risk

## Context

`crates/acp` ports `others/acpx` (a TypeScript ACP CLI built on `@agentclientprotocol/sdk`)
into Rust. The TS SDK gives acpx JSON-RPC 2.0 framing, protocol types, and a
`ClientSideConnection` for free. Research prior to implementation found an
official Rust SDK — `agent-client-protocol` + `agent-client-protocol-schema`
(github.com/agentclientprotocol/rust-sdk, used by Zed) — but flagged that
`session/set_mode`, `session/set_config_option`, and `terminal/release` were
not yet exposed as typed Rust request/response structs in the version
available at research time.

## Decision

Reuse `agent-client-protocol` 1.0.1 / `agent-client-protocol-schema` 1.1.0
(exact-pinned in `[workspace.dependencies]`) for everything they cover:
`initialize`, `session/new`, `session/prompt`, `session/cancel`, `fs/*`,
`terminal/create|output|kill`, `request_permission`. For the 3 methods
research flagged as missing, add a thin `jsonrpc_gap.rs` module.

**Implementation outcome (discovered during Phase 2, revises the research
finding above):** direct inspection of the vendored SDK source showed all
three methods are, in fact, already fully typed via the SDK's
`impl_jsonrpc_request!` machinery — `SetSessionModeRequest`/`Response` and
`SetSessionConfigOptionRequest`/`Response` in `schema::v1::client_to_agent`,
`ReleaseTerminalRequest`/`Response` in `schema::v1::agent_to_client`.
`jsonrpc_gap.rs` ended up as a documented pass-through re-export instead of
hand-rolled parallel structs — the originally-planned "raw-request escape
hatch" contingency was unnecessary because `ConnectionTo::send_request`
already accepts any `JsonRpcRequest` type, typed or not.

## Alternatives Considered

- **Hand-roll the entire protocol.** Would duplicate ~500-800 LOC of JSON-RPC
  framing and type definitions the SDK already provides, violating DRY and
  diverging from the reference Rust implementation Zed ships.
- **Wait on upstream to add the 3 originally-missing methods.** No committed
  timeline; blocks the port indefinitely. Moot once inspection showed they
  already existed.
- **Use `agent-client-protocol-tokio`.** Hardcodes tokio, which this
  workspace does not use (see [0002](./0002-async-substrate-smol.md)).

## Consequences

- The crate's protocol layer tracks an externally-maintained SDK's release
  cadence; version bumps need deliberate review (pinned with `=` in
  `Cargo.toml`, not a caret range).
- No wire-format drift risk versus the reference Rust ACP implementation.
- If a future ACP spec revision genuinely lacks Rust-side typed bindings,
  `jsonrpc_gap.rs` is the established extension point to hand-roll just the
  gap, without touching the rest of the protocol layer.
