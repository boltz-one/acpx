# 0006. Permission-request API is async/channel-based, never blocks the GPUI event loop

- **Status:** accepted
- **Date:** 2026-07-05
- **Lane:** high-risk

## Context

acpx's `permission-prompt.ts` calls `promptForPermission()`, a
synchronous-feeling stdin read gated by `process.stdin.isTTY &&
process.stderr.isTTY`. A GUI has no stdin TTY, and more importantly, GPUI's
render loop must never block on user input arriving from an arbitrary
future — an interactive permission prompt in a GUI is inherently a "show a
dialog, wait for a click, resume the agent's pending RPC response" flow that
can take seconds or indefinitely long.

## Decision

Define `trait PermissionRequestHandler: Send + Sync` with an async
`request(&self, req: PermissionRequestParams) -> BoxFuture<'static,
PermissionDecision>` method. The runtime is constructed with an
`Arc<dyn PermissionRequestHandler>`. When a decision needs interactive
input, the permissions module calls `handler.request(...)` and awaits it —
this await point suspends only the specific in-flight `session/prompt`
RPC's response, not the GPUI event loop, because it runs on the
`smol`-driven task from [0002](./0002-async-substrate-smol.md), not on
GPUI's UI thread. A GPUI app's implementation sends the request over a
channel to the UI, shows a dialog, and resolves a
`futures::channel::oneshot::Sender` when the user responds.

Filesystem and terminal permission prompts (acpx had bespoke
`confirmWrite`/`confirmExecute` stdin-prompt callbacks for each) were unified
onto this same handler via a shared `permissions::confirm_action` helper
that builds a synthetic allow/reject `RequestPermissionRequest` — one
interactive-decision mechanism crate-wide instead of three divergent shapes.

## Alternatives Considered

- **A synchronous callback** (`Fn(...) -> PermissionDecision`, no `async`).
  Would force the caller — deep inside the transport read loop — to block a
  thread waiting for UI interaction, either freezing that smol worker or, if
  it also drives other sessions' I/O, stalling unrelated sessions too.
- **A polling API** (`fn poll_decision() -> Option<PermissionDecision>`,
  called repeatedly). Pushes complexity onto every caller and reintroduces
  stdin-prompt-style busy-waiting antipatterns.

## Consequences

- Matches acpx's own `onPermissionRequest` callback shape
  (`Promise<AcpPermissionDecision | undefined>`) almost exactly — same
  conceptual API, made non-blocking-safe for the host runtime it now lives
  in, minimizing behavioral surprise for anyone who knows the TS contract.
- A test proves the non-blocking property directly: a pending permission
  decision does not prevent a second, unrelated future on the same executor
  from making progress.
- acpx's escalation-audit metadata is surfaced via a typed
  `ResolvedPermissionRequest.escalation` field rather than duplicated into a
  response's `_meta.acpx.permissionEscalation` blob — simpler for Rust
  callers, same audit information.
