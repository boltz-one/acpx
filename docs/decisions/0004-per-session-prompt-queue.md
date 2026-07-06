# 0004. Prompt queueing is per-session, not per-client or global

- **Status:** accepted
- **Date:** 2026-07-05
- **Lane:** high-risk

## Context

acpx's base client has one active-prompt slot per `AcpClient` instance
(effectively per-agent-process, since one client = one spawned agent
subprocess in the CLI's usage model) — not a real queue: a second prompt
while one is active is rejected, not enqueued. acpx's CLI layer builds actual
cross-process queueing (`cli/queue/{ipc-server,lease-store,
owner-turn-controller}.ts`) on top of this because its use case is many
separate `acpx` process invocations against one long-lived owner daemon —
explicitly out of scope for this port (a GPUI app is one long-running
process, not many short-lived CLI invocations).

The GUI's actual concurrency need is different from both acpx layers: one
process, potentially many concurrent ACP sessions (e.g. multiple chat tabs),
where each *session* should still behave like acpx's client (one active
prompt at a time, ordered), but unrelated sessions must not block each other.

## Decision

Move the single-flight slot from "per client" to "per session"
(`queue::SessionPromptQueue`, keyed by session id, each with its own slot + a
small bounded FIFO, default capacity 4, configurable via
`SessionPromptQueue::with_capacity(n)`). A prompt request for session A never
waits on session B's queue. Within one session, ordering and single-flight
semantics match acpx's real guarantee exactly.

`cancel_active(session_id)` cancels only the running turn and does **not**
clear queued-but-not-started requests by default (least-surprise: cancelling
"this" turn shouldn't silently discard a user's next already-submitted
message). A separate `clear_queue`/`cancel_active_and_clear` is available for
an explicit "stop everything" UI action.

Per-session `session/update` notification ordering uses a **separate** lock
from the prompt-queue lock (`connected_session.rs`'s `update_order` mutex) —
the two locks must never nest, to avoid a deadlock between dispatch and
update-ordering.

## Alternatives Considered

- **Keep acpx's exact per-client behavior.** Would mean opening a chat tab
  against agent X while another tab is mid-turn on agent X blocks the second
  tab entirely, with no user mental model of "these tabs share a queue" —
  poor UX for a GUI (vs. a scripted CLI pipeline where serialization was an
  intentional simplicity trade-off).
- **A single global queue across all sessions/clients.** Strictly worse than
  per-session for a multi-session GUI; also loses the one good reason acpx's
  design has (backpressure against one script hammering one CLI invocation),
  which doesn't apply to a GUI issuing user-paced requests.
- **No queueing at all — reject concurrent prompts on the same session
  outright.** Simpler, but a real GUI can plausibly fire two `start_turn`
  calls in quick succession for the same session (fast double-submit, a
  "regenerate" issued before the prior turn's UI settles); a small bounded
  FIFO absorbs this without every caller needing its own debounce logic.

## Consequences

- `AcpRuntime::start_turn` is the actual queueing entry point;
  `prompt_turn::run_turn` became an internal implementation detail the
  dispatcher calls, not something external callers invoke directly — a
  planned, sanctioned adjustment to Phase 4's initial public contract.
- Exceeding a session's queue bound returns a specific
  `AcpRuntimeErrorCode::TurnQueueFull` result, not a panic or silent drop,
  and does not affect other sessions.
- A UI that needs different-per-session queue depths can pass
  `AcpRuntimeOptions::prompt_queue_capacity` per session at construction time.
