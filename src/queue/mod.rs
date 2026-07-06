//! Per-session bounded FIFO prompt queue (ADR-4, Phase 6). See
//! `plans/20260705-1718-acpx-to-acp-crate-port/phase-06-prompt-queueing-cancellation.md`
//! for the full requirements/architecture this module implements; not a
//! 1:1 port of anything in `others/acpx` — acpx's cross-process CLI queue
//! (`cli/queue/*`) is out of scope, and even its in-process client only has
//! a single-slot "reject, don't queue" guard (see `others/acpx/src/acp/client.ts`'s
//! `activePrompt`/`hasActivePrompt`). This is a from-scratch, purpose-built
//! design for the GUI's actual concurrency need (ADR-4's rationale).
//!
//! [`SessionPromptQueue`] is the module's only public type: one per ACP
//! session (owned by `runtime::engine::connected_session::ConnectedSession`'s
//! `prompt_queue` field), giving each session its own single-flight slot +
//! bounded FIFO so a slow/stuck prompt on one session never delays another
//! (Requirement 2) — sessions never share a `SessionPromptQueue` or its
//! internal lock, so there is no cross-session contention point at all.

mod dispatcher;
mod session_queue;

use std::sync::Arc;

use futures::channel::oneshot;

use crate::runtime::engine::connected_session::ConnectedSession;
use crate::runtime::public::contract::{AcpRuntimeTurn, AcpRuntimeTurnInput, AcpSessionStore};
use crate::runtime::public::errors::{AcpRuntimeError, AcpRuntimeErrorCode};

use dispatcher::{PendingPromptRequest, Queue, dispatch};
use session_queue::Admission;

/// Default bound on a session's pending-prompt FIFO (Requirement 1/Step 3).
/// Sized for "a fast double-submit or a stop-then-regenerate UI flow", not
/// sustained rapid-fire usage — a GUI issuing more than a handful of
/// un-awaited prompts against one session before the first even starts
/// almost certainly has its own bug upstream. Override via
/// [`SessionPromptQueue::with_capacity`] (wired through
/// `AcpRuntimeOptions::prompt_queue_capacity`).
pub const DEFAULT_QUEUE_CAPACITY: usize = 4;

/// Errors [`SessionPromptQueue::enqueue`] can report. Kept distinct from
/// [`AcpRuntimeError`] so a caller that cares can match on queue-specific
/// outcomes without string-matching a message; `AcpRuntime::start_turn`
/// converts this into an `AcpRuntimeError` (see the `From` impl below) to
/// preserve its own "always returns an `AcpRuntimeTurn`, never a bare
/// `Result`" convention (errors surface as an already-failed turn).
#[derive(Debug, thiserror::Error)]
pub enum SessionPromptQueueError {
    /// Requirement 1's bounded backpressure: the FIFO was already full.
    /// Not a panic, not a silently dropped request (Success Criteria #3).
    #[error("session prompt queue is full ({pending} pending, capacity {capacity})")]
    QueueFull { pending: usize, capacity: usize },
    /// The request was still queued (not yet dispatched) when
    /// [`SessionPromptQueue::clear_queue`]'s clear ran (Step 6's explicit
    /// "stop everything" opt-in, separate from cancelling the active turn).
    #[error("request was cleared from the session prompt queue before it started")]
    Cleared,
    /// `prompt_turn::start_turn` itself failed synchronously (e.g. an
    /// unsupported attachment) before any turn ever began.
    #[error(transparent)]
    TurnFailed(#[from] AcpRuntimeError),
}

impl From<SessionPromptQueueError> for AcpRuntimeError {
    fn from(err: SessionPromptQueueError) -> Self {
        match err {
            SessionPromptQueueError::TurnFailed(err) => err,
            SessionPromptQueueError::QueueFull { pending, capacity } => AcpRuntimeError::new(
                AcpRuntimeErrorCode::TurnQueueFull,
                format!("session prompt queue is full ({pending} pending, capacity {capacity})"),
            ),
            SessionPromptQueueError::Cleared => AcpRuntimeError::new(
                AcpRuntimeErrorCode::TurnFailed,
                "request was cleared from the session prompt queue before it started",
            ),
        }
    }
}

/// One session's single-flight slot + bounded FIFO (Requirement 1, ADR-4).
/// See the module docs for the state machine and locking invariant.
pub struct SessionPromptQueue {
    state: Arc<Queue>,
}

impl SessionPromptQueue {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_QUEUE_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            state: Arc::new(Queue::new(capacity)),
        }
    }

    pub fn capacity(&self) -> usize {
        self.state.capacity()
    }

    /// Number of requests queued (not counting the currently running one,
    /// if any).
    pub fn queue_len(&self) -> usize {
        self.state.pending_len()
    }

    /// Enqueues a prompt request. Resolves once it is this request's turn:
    /// immediately if the session was idle (matching acpx's
    /// synchronous-looking `startTurn`, Step 2), or after every
    /// earlier-submitted request on this session completes (Success
    /// Criteria #2 — same-session FIFO ordering, submission order
    /// preserved). Returns `Err(QueueFull)` synchronously, without waiting
    /// at all, if the bounded FIFO is already at capacity (Success Criteria
    /// #3).
    pub async fn enqueue(
        &self,
        connected: Arc<ConnectedSession>,
        session_store: Arc<dyn AcpSessionStore>,
        input: AcpRuntimeTurnInput,
        default_timeout_ms: Option<u64>,
    ) -> Result<AcpRuntimeTurn, SessionPromptQueueError> {
        let (started_tx, started_rx) = oneshot::channel();
        let request = PendingPromptRequest {
            connected,
            session_store,
            input,
            default_timeout_ms,
            started_tx,
        };

        match self.state.try_admit(request) {
            Admission::RunNow(request) => dispatch(self.state.clone(), request),
            Admission::Queued => {}
            Admission::Rejected(_request) => {
                return Err(SessionPromptQueueError::QueueFull {
                    pending: self.queue_len(),
                    capacity: self.capacity(),
                });
            }
        }

        started_rx
            .await
            .map_err(|_| SessionPromptQueueError::Cleared)?
            .map_err(SessionPromptQueueError::TurnFailed)
    }

    /// Drops every queued-but-not-yet-started request for this session,
    /// leaving the currently running turn (if any) untouched — Step 6's
    /// documented default: cancelling "this" turn does not silently
    /// discard an already-submitted next message; this is the separate,
    /// explicit "also drop pending requests" call for a "stop everything"
    /// UI action. Returns how many requests were cleared.
    pub fn clear_queue(&self) -> usize {
        self.state.clear_pending().len()
    }
}

impl Default for SessionPromptQueue {
    fn default() -> Self {
        Self::new()
    }
}
