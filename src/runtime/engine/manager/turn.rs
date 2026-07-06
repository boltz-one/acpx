//! `AcpRuntime::{start_turn, run_turn}`: turn dispatch through the
//! per-session prompt queue, plus the legacy `run_turn` compatibility
//! adapter. Split out of `manager/mod.rs` per the workspace's per-file line
//! convention — see that module's docs for the split rationale.

use futures::StreamExt;
use futures::stream::BoxStream;

use super::AcpRuntime;
use crate::runtime::engine::manager_support::failed_turn;
use crate::runtime::public::contract::{
    AcpRuntimeTurn, AcpRuntimeTurnInput, legacy_terminal_event_from_turn_result,
};
use crate::runtime::public::events::AcpRuntimeEvent;

impl AcpRuntime {
    /// Ports `startTurn` (ADR-7). Phase 6 (ADR-4) adjustment: this is now
    /// `async` — it enqueues onto the session's `SessionPromptQueue`
    /// (`connected_session.rs`'s `prompt_queue` field) rather than calling
    /// `engine::prompt_turn::start_turn` directly, and only resolves once
    /// it's this request's turn to actually run (immediately if the
    /// session was idle, matching acpx's synchronous-looking `startTurn`
    /// shape; after earlier same-session requests complete otherwise). See
    /// `crate::queue`'s module docs for the full design. This is the
    /// sanctioned Phase 4 API adjustment flagged in this phase's Risk
    /// Assessment ("if Phase 4 ships `prompt_turn::run_turn` as the only
    /// entry point... the public `AcpRuntime::start_turn` signature needs a
    /// follow-up change") — `prompt_turn::start_turn` is now an
    /// implementation detail `crate::queue::dispatcher` calls, not
    /// something this method invokes directly.
    pub async fn start_turn(&self, input: AcpRuntimeTurnInput) -> AcpRuntimeTurn {
        let request_id = input.request_id.clone();
        let connected = match self.connected(&input.handle) {
            Ok(connected) => connected,
            Err(err) => return failed_turn(request_id, err),
        };
        let timeout_ms = self.options.timeout_ms;
        let session_store = self.options.session_store.clone();
        match connected
            .prompt_queue
            .enqueue(connected.clone(), session_store, input, timeout_ms)
            .await
        {
            Ok(turn) => turn,
            Err(err) => failed_turn(request_id, err.into()),
        }
    }

    /// Ports `runTurn`: the compatibility adapter that folds the terminal
    /// result back into the event stream as a `done`/`error` event. Prefer
    /// [`Self::start_turn`]. `async` for the same reason as `start_turn`.
    pub async fn run_turn(
        &self,
        input: AcpRuntimeTurnInput,
    ) -> BoxStream<'static, AcpRuntimeEvent> {
        let mut turn = self.start_turn(input).await;
        let events = turn.events();
        let result = turn.result();
        let terminal =
            futures::stream::once(
                async move { legacy_terminal_event_from_turn_result(&result.await) },
            );
        Box::pin(events.chain(terminal))
    }
}
