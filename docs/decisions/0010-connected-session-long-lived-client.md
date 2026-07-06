# 0010. `ConnectedSession` keeps one long-lived client per session, not one ephemeral client per turn

- **Status:** accepted
- **Date:** 2026-07-06
- **Lane:** normal

## Context

acpx (the TypeScript original this crate ports) is invoked fresh from the
command line for every user turn: each invocation spins up a new agent
subprocess/client connection, runs exactly one turn, and tears the
connection down when the CLI process exits. Because that per-invocation
teardown is unavoidable overhead, acpx layers a connection-pooling scheme on
top so a *logical* session can reuse a warm subprocess across successive
CLI invocations rather than paying full spawn cost every turn — the pool is
a workaround for a process model that is inherently short-lived per call.

This Rust port's host is different: a single, continuously-running process
(the GPUI app) that keeps a session open for as long as the user has the
corresponding session/tab open. `runtime::engine::connected_session::ConnectedSession`
(ported across the original Phases 4/6) reflects that: it owns one live
[`AcpClient`], the backend session id, the mutable [`SessionRecord`], the
in-memory `SessionConversation`, and the `session/update` notification feed
for the *entire lifetime of the session*, not just for the duration of one
turn. `runtime::engine::manager::mod.rs`'s `ensure_session` reuses this same
`ConnectedSession` across turns whenever its agent process is still alive
(see `stored_process_status`/gap 33), reconnecting only on demand — when the
in-memory entry is missing or its process has died — rather than on every
turn. This was an implicit consequence of how the manager/reconnect state
machine was designed, not a decision anyone wrote down as a deliberate,
top-level architectural choice — the audit that produced this ADR treats
that omission as itself a gap, since an undocumented simplification looks
identical to an accidental one from the outside.

## Decision

Keep `ConnectedSession`'s long-lived-client-per-session model exactly as
built: one live `AcpClient` per session, held open until the session is
explicitly closed or a reconnect is forced by a detected process death, with
no per-turn teardown/respawn and no connection pool. This is a deliberate,
accepted deviation from acpx's ephemeral-client-per-turn + connection-pooling
model, not an oversight.

## Alternatives Considered

- **Match acpx's ephemeral-client-per-turn model exactly** (tear down and
  respawn the agent subprocess after every turn). Rejected: acpx's model
  exists to cope with its CLI-per-invocation process lifecycle, where the
  *host* process itself is inherently short-lived and per-turn respawn is
  the only way to reuse anything at all across separate CLI invocations. In
  a single long-running GPUI process, there is no equivalent forcing
  function — the session's `ConnectedSession` naturally lives exactly as
  long as the GUI session/tab is open, making per-turn teardown/respawn pure
  subprocess-spawn overhead with no corresponding benefit.
- **Port acpx's connection-pooling layer as-is.** Rejected as redundant
  scaffolding: the pool's entire purpose is to let an inherently ephemeral
  process model *simulate* a long-lived connection across invocations. This
  port's host already provides a genuinely long-lived process, so building
  a pool on top of it would just be re-deriving "one connection, kept alive"
  through an extra layer of indirection that has no failure mode of its own
  to justify the added complexity (YAGNI).

## Consequences

- Turn latency after the first turn in a session has no subprocess-spawn
  cost, unlike a strict per-turn-respawn model — a direct benefit of not
  matching acpx's ephemeral model.
- The manager (`runtime::engine::manager`) must itself detect a dead agent
  process before reusing a `ConnectedSession` (see gap 33's
  `reconnect::liveness::stored_process_status` consolidation) — a
  responsibility acpx's pool would otherwise have absorbed. This crate's
  reconnect state machine (`runtime::engine::reconnect`) exists specifically
  to own that responsibility.
- A `ConnectedSession` alive with no active turn still holds one open agent
  subprocess and its stdio pipes for the whole time the session/tab remains
  open — memory/handle cost acpx's ephemeral model would not pay between
  turns. Acceptable trade-off: this crate's host is a desktop app with a
  small number of concurrently open sessions, not a server fanning out to
  many concurrent short-lived CLI invocations.
