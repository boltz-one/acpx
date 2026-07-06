//! Wires [`super::session_queue::SessionQueueState`]'s admission decisions
//! to the real turn machinery: [`crate::runtime::engine::prompt_turn::start_turn`].
//! This is the module the phase's Architecture section calls out as
//! "drains one session's queue, invokes Phase 4's prompt_turn::run_turn,
//! re-arms the slot on completion, wakes the next queued request" (Phase 4
//! landed that function as `start_turn`, not `run_turn` — same function,
//! the plan predates the exact name).

use std::sync::Arc;

use futures::channel::oneshot;

use crate::runtime::engine::connected_session::ConnectedSession;
use crate::runtime::engine::prompt_turn;
use crate::runtime::public::contract::{AcpRuntimeTurn, AcpRuntimeTurnInput, AcpSessionStore};
use crate::runtime::public::errors::AcpRuntimeError;

use super::session_queue::SessionQueueState;

/// One admitted-or-queued prompt request: everything
/// `prompt_turn::start_turn` needs, plus the "your turn has started" signal
/// (Step 2) that [`super::SessionPromptQueue::enqueue`]'s returned future is
/// awaiting.
pub(super) struct PendingPromptRequest {
    pub(super) connected: Arc<ConnectedSession>,
    pub(super) session_store: Arc<dyn AcpSessionStore>,
    pub(super) input: AcpRuntimeTurnInput,
    pub(super) default_timeout_ms: Option<u64>,
    pub(super) started_tx: oneshot::Sender<Result<AcpRuntimeTurn, AcpRuntimeError>>,
}

pub(super) type Queue = SessionQueueState<PendingPromptRequest>;

/// Runs `request` now: calls `prompt_turn::start_turn`, immediately reports
/// the (possibly failed) outcome to the waiting caller, and — on the
/// success path — spawns a task that waits for the turn to actually finish
/// before re-arming the queue via [`advance`].
pub(super) fn dispatch(queue: Arc<Queue>, request: PendingPromptRequest) {
    let PendingPromptRequest {
        connected,
        session_store,
        input,
        default_timeout_ms,
        started_tx,
    } = request;

    let (freed_tx, freed_rx) = oneshot::channel::<()>();
    let outcome = prompt_turn::start_turn(
        connected,
        session_store,
        input,
        default_timeout_ms,
        Some(freed_tx),
    );
    let started_ok = outcome.is_ok();
    // Best-effort: if nobody is awaiting `enqueue()` any more (e.g. the
    // caller dropped the future), there's nothing left to report to.
    let _ = started_tx.send(outcome);

    if !started_ok {
        // `start_turn` failed synchronously (e.g. an unsupported attachment
        // media type) before spawning any background task, so `freed_tx`
        // will never fire. Free the slot right here rather than awaiting a
        // receiver that would otherwise hang forever.
        advance(queue);
        return;
    }

    smol::spawn(async move {
        // Cleared (Err) or fired (Ok) both mean "the slot is free now" —
        // the only way this resolves to Err is the sender being dropped
        // without sending, which `prompt_turn::run_turn_task` never does on
        // any of its exit paths (success, RPC error, timeout, or
        // cancellation all reach the `on_slot_freed` send).
        let _ = freed_rx.await;
        advance(queue);
    })
    .detach();
}

/// The running payload just finished (or never really started — see
/// [`dispatch`]'s synchronous-failure branch); pop the next queued request,
/// if any, and dispatch it, preserving FIFO submission order.
fn advance(queue: Arc<Queue>) {
    if let Some(next) = queue.free_slot() {
        dispatch(queue, next);
    }
}
