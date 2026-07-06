//! `AcpRuntimeTurn`: the one type in [`super::types`]'s family with real
//! behavior (a live event stream plus a one-shot terminal result and
//! cancellation), split out on its own for that reason.

use futures::future::BoxFuture;
use futures::stream::BoxStream;

use crate::runtime::public::events::AcpRuntimeEvent;

use super::types::AcpRuntimeTurnResult;

/// Ports `AcpRuntimeTurn` (ADR-7): `events` is a pull-based `Stream` rather
/// than acpx's `AsyncIterable`, and `result` is a boxed future settled
/// exactly once the turn reaches a terminal state, kept separate from the
/// live event stream per acpx's own already-improved `startTurn` shape (see
/// the `done`/`error` doc comments on [`AcpRuntimeEvent`]).
pub struct AcpRuntimeTurn {
    pub request_id: String,
    events: Option<BoxStream<'static, AcpRuntimeEvent>>,
    result: BoxFuture<'static, AcpRuntimeTurnResult>,
    cancel_tx: Option<futures::channel::oneshot::Sender<Option<String>>>,
}

impl AcpRuntimeTurn {
    pub(crate) fn new(
        request_id: String,
        events: BoxStream<'static, AcpRuntimeEvent>,
        result: BoxFuture<'static, AcpRuntimeTurnResult>,
        cancel_tx: futures::channel::oneshot::Sender<Option<String>>,
    ) -> Self {
        Self {
            request_id,
            events: Some(events),
            result,
            cancel_tx: Some(cancel_tx),
        }
    }

    /// Takes the live event stream. Ports the `events` getter; a `take`
    /// rather than a `&self` borrow because `BoxStream` isn't `Clone` and
    /// acpx's own consumer only ever drains it once.
    pub fn events(&mut self) -> BoxStream<'static, AcpRuntimeEvent> {
        self.events
            .take()
            .unwrap_or_else(|| Box::pin(futures::stream::empty()))
    }

    /// Awaits the turn's terminal result. Ports the `result` getter.
    pub fn result(self) -> BoxFuture<'static, AcpRuntimeTurnResult> {
        self.result
    }

    /// Ports `cancel(...)`: requests cancellation; the turn's task sends
    /// `session/cancel` and keeps awaiting the agent's (now-cancelled)
    /// response rather than tearing down early, per ACP's cancellation
    /// protocol (the agent MUST still reply with `stopReason: cancelled`).
    pub fn cancel(&mut self, reason: Option<String>) {
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(reason);
        }
    }

    /// Ports `closeStream(...)`: stops draining live events early (e.g. the
    /// UI navigated away) without affecting the in-flight turn itself. The
    /// producer side uses a bounded, best-effort `try_send` (see
    /// `engine::prompt_turn`), so it never blocks even if nothing drains
    /// the stream after this is called.
    pub fn close_stream(&mut self) {
        self.events = None;
    }
}
