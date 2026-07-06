//! Ports `others/acpx/src/runtime/engine/connected-session.ts`: owns one
//! live [`AcpClient`] + backend session id pairing, plus the mutable
//! session-scoped state ([`crate::session::record::SessionRecord`], the
//! in-memory [`SessionConversation`], and the raw `session/update`
//! notification feed) a running turn or a `get_status`/`set_mode` call
//! reads or updates.
//!
//! Per Requirement 6 and this file's module docs in the phase plan,
//! `has_active_prompt`/`request_cancel_active_prompt` here describe *this
//! session's* in-flight turn only (an `AtomicBool` this struct owns), not a
//! client-global single-flight lock — Phase 6's per-session queue builds on
//! top of this, it does not replace it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use agent_client_protocol::schema::v1::{
    McpServer, SessionConfigId, SessionConfigValueId, SessionId, SessionModeId,
    SessionNotification, SetSessionConfigOptionResponse,
};
use parking_lot::Mutex;

use crate::client::AcpClient;
use crate::error::Result;
use crate::queue::SessionPromptQueue;
use crate::session::conversation_model::SessionConversation;
use crate::session::model_state::advertised_model_state;
use crate::session::record::SessionRecord;

/// One connected ACP session: the live client, the backend session id
/// (mutable — reconnect may create a fresh one), and the record/conversation
/// state a turn mutates as it runs.
pub struct ConnectedSession {
    pub client: AcpClient,
    session_id: Mutex<SessionId>,
    pub record: Mutex<SessionRecord>,
    pub conversation: Mutex<SessionConversation>,
    /// Fed by the client's `session/update` notification handler (see
    /// `crate::client::handlers::ClientRequestHandlers`); cloned into
    /// whichever turn is currently draining it (see module docs — this
    /// phase assumes at most one active turn per session in practice).
    pub notifications: smol::channel::Receiver<SessionNotification>,
    /// Gap 20: fed by the filesystem/terminal handlers' `on_operation`
    /// callbacks (attached in `manager_spawn.rs`). The active turn task
    /// drains this, persisting each op via `record_client_operation` and
    /// streaming an [`crate::runtime::public::events::AcpRuntimeEvent::ClientOperation`].
    /// Client operations only occur during a prompt turn (agent tool use),
    /// so this channel stays empty between turns.
    pub operations: smol::channel::Receiver<crate::filesystem::ClientOperation>,
    pub mcp_servers: Vec<McpServer>,
    active_prompt: AtomicBool,
    /// Phase 6 (ADR-4): this session's bounded FIFO prompt queue. Public so
    /// `runtime::engine::manager::AcpRuntime::start_turn` can enqueue
    /// through it directly instead of calling `prompt_turn::start_turn`
    /// itself (that call is now `crate::queue::dispatcher`'s job).
    pub prompt_queue: SessionPromptQueue,
    /// Serializes `session/update` notification application in arrival
    /// order (Requirement 4; ports acpx's `sessionUpdateChain` guarantee at
    /// session granularity). Use [`Self::with_ordered_update`] rather than
    /// reaching in directly.
    ///
    /// This is a SEPARATE lock from `prompt_queue`'s internal admission
    /// lock (Risk Assessment: "deadlock risk between the per-session
    /// update-ordering lock and the prompt-queue lock") — code in this
    /// crate must never hold both at once. In practice `prompt_queue` never
    /// touches `update_order` and vice versa (neither module even imports
    /// the other's lock type), so the invariant holds by construction, not
    /// just by convention; keep it that way in any future change.
    update_order: Mutex<()>,
    /// Gap 16: how many `session/update` notifications arrived on
    /// [`Self::notifications`]'s channel in total, versus how many a live
    /// consumer actually drained/processed. Ports acpx's
    /// `observedSessionUpdates`/`processedSessionUpdates` counters
    /// (`waitForSessionUpdateDrain`'s inputs), seeded from the load-path
    /// suppression drain performed before this `ConnectedSession` is
    /// constructed (see [`crate::runtime::engine::connected_session::drain_replay_notifications`]
    /// and `manager_spawn.rs`'s call site) — see [`Self::session_update_counts`]'s
    /// doc comment for why a channel-based drain replaces acpx's wall-clock
    /// idle wait here.
    observed_session_updates: AtomicU64,
    processed_session_updates: AtomicU64,
}

impl ConnectedSession {
    /// `suppressed_replay_updates` (gap 16): how many `session/update`
    /// notifications were drained and discarded (via
    /// [`drain_replay_notifications`]) from `notifications`'s channel
    /// *before* this `ConnectedSession` — and therefore before any live
    /// consumer — was constructed, during a `session/load`/`session/resume`
    /// reconnect. Seeds [`Self::observed_session_updates`] so the count is
    /// visible for diagnostics/tests without a live consumer ever having
    /// seen those replay updates.
    pub fn new(
        client: AcpClient,
        session_id: SessionId,
        record: SessionRecord,
        conversation: SessionConversation,
        notifications: smol::channel::Receiver<SessionNotification>,
        operations: smol::channel::Receiver<crate::filesystem::ClientOperation>,
        mcp_servers: Vec<McpServer>,
        prompt_queue_capacity: Option<usize>,
        suppressed_replay_updates: u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            client,
            session_id: Mutex::new(session_id),
            record: Mutex::new(record),
            conversation: Mutex::new(conversation),
            notifications,
            operations,
            mcp_servers,
            active_prompt: AtomicBool::new(false),
            prompt_queue: match prompt_queue_capacity {
                Some(capacity) => SessionPromptQueue::with_capacity(capacity),
                None => SessionPromptQueue::new(),
            },
            update_order: Mutex::new(()),
            observed_session_updates: AtomicU64::new(suppressed_replay_updates),
            processed_session_updates: AtomicU64::new(0),
        })
    }

    /// Gap 16 introspection: `(observed, processed)` — `observed` includes
    /// any replay updates suppressed/discarded during the connect-time load
    /// drain (see [`Self::new`]'s `suppressed_replay_updates`); `processed`
    /// tracks how many a live consumer has actually drained via
    /// [`Self::record_processed_update`]. Mostly useful for diagnostics and
    /// tests confirming the load-path drain actually ran.
    pub fn session_update_counts(&self) -> (u64, u64) {
        (
            self.observed_session_updates.load(Ordering::SeqCst),
            self.processed_session_updates.load(Ordering::SeqCst),
        )
    }

    /// Call when a live consumer pulls one notification off
    /// [`Self::notifications`] and actually processes it (as opposed to a
    /// replay update discarded before this session existed). Not currently
    /// called by this phase's owned files — the actual drain loop lives in
    /// `prompt_turn/task.rs`, outside this phase's file scope — kept as
    /// public API so that file (or a future phase) can opt in without
    /// another `ConnectedSession` field addition.
    pub fn record_processed_update(&self) {
        self.processed_session_updates
            .fetch_add(1, Ordering::SeqCst);
        self.observed_session_updates.fetch_add(1, Ordering::SeqCst);
    }

    /// Runs `f` (expected to be a quick, non-blocking append/forward — see
    /// Step 5's design note) under the `session/update` ordering lock. The
    /// caller's per-session single `notifications` consumer loop already
    /// only ever runs one `f` at a time in arrival order; this lock makes
    /// that guarantee explicit and future-proof rather than an incidental
    /// side effect of today's single-consumer structure.
    pub(crate) fn with_ordered_update<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = self.update_order.lock();
        f()
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id.lock().clone()
    }

    pub fn set_session_id(&self, session_id: SessionId) {
        *self.session_id.lock() = session_id;
    }

    /// Ports `hasActivePrompt` (session-scoped, see module docs).
    pub fn has_active_prompt(&self) -> bool {
        self.active_prompt.load(Ordering::SeqCst)
    }

    pub(crate) fn set_active_prompt(&self, active: bool) {
        self.active_prompt.store(active, Ordering::SeqCst);
    }

    /// Ports `requestCancelActivePrompt`: sends `session/cancel` only if a
    /// prompt is actually in flight, returning whether it did.
    pub fn request_cancel_active_prompt(&self) -> Result<bool> {
        if !self.has_active_prompt() {
            return Ok(false);
        }
        self.client.cancel_session(self.session_id())?;
        Ok(true)
    }

    /// Ports `setSessionMode`.
    pub async fn set_session_mode(&self, mode_id: &str) -> Result<()> {
        self.client
            .set_session_mode(self.session_id(), SessionModeId::new(mode_id))
            .await?;
        Ok(())
    }

    /// Ports `setSessionModel`: resolves the advertised model config id from
    /// the record's `acpx` state, then delegates to `session/set_config_option`.
    pub async fn set_session_model(
        &self,
        model_id: &str,
    ) -> Result<SetSessionConfigOptionResponse> {
        let config_id = {
            let record = self.record.lock();
            advertised_model_state(record.acpx.as_ref()).and_then(|models| models.config_id)
        };
        let config_id = config_id.unwrap_or_else(|| "model".to_string());
        self.client
            .set_session_config_option(
                self.session_id(),
                SessionConfigId::new(config_id),
                SessionConfigValueId::new(model_id),
            )
            .await
    }

    /// Ports `setSessionConfigOption`.
    pub async fn set_session_config_option(
        &self,
        config_id: &str,
        value: &str,
    ) -> Result<SetSessionConfigOptionResponse> {
        self.client
            .set_session_config_option(
                self.session_id(),
                SessionConfigId::new(config_id),
                SessionConfigValueId::new(value),
            )
            .await
    }
}

/// Gap 16 (ADR): the load-path replay-suppression/drain mechanism. acpx's
/// `waitForSessionUpdateDrain` polls two counters against a wall-clock idle
/// timer because Node's single-threaded event loop has no cheaper way to
/// answer "is anything still in flight". This crate instead drains the
/// notification channel synchronously here, at connect time, before any live
/// consumer holds the receiver (`ConnectedSession::new` runs after this) —
/// discarding the historical `session/update`s an agent replays during a
/// `session/resume`/`session/load` so they aren't re-forwarded to the live
/// event stream. Returns how many notifications were discarded.
///
/// **Known residual race (accepted, low-probability):** the background
/// connection task's notification closure runs on a *separate* task, so an
/// update the agent emits *spontaneously* in the narrow window between the
/// acquisition RPC's response resolving and this synchronous drain executing
/// would also be discarded — this sweep cannot distinguish "stale replay"
/// from "a genuinely-live update that just raced in". In practice a resumed
/// agent's replay history is sent *during* the RPC (already buffered before
/// the response resolves, so correctly suppressed), and an unsolicited
/// out-of-turn update in that microsecond window is rare and re-synced on
/// the next turn — hence accepted rather than blocking. Fully closing it
/// would require phase-04's originally-specified receive-time suppression
/// flag (consulted by `handshake.rs`'s `on_receive_notification` closure,
/// set during load, cleared after drain); left as a documented follow-up
/// since the failure mode is a rare, recoverable lost UI update, not an
/// authorization or data-integrity fault.
pub fn drain_replay_notifications(rx: &smol::channel::Receiver<SessionNotification>) -> u64 {
    let mut drained = 0u64;
    while rx.try_recv().is_ok() {
        drained += 1;
    }
    drained
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_replay_notifications_discards_buffered_messages_and_reports_count() {
        let (tx, rx) = smol::channel::unbounded();
        for i in 0..3 {
            tx.try_send(SessionNotification::new(
                SessionId::new(format!("s-{i}")),
                agent_client_protocol::schema::v1::SessionUpdate::UserMessageChunk(
                    agent_client_protocol::schema::v1::ContentChunk::new(
                        agent_client_protocol::schema::v1::ContentBlock::Text(
                            agent_client_protocol::schema::v1::TextContent::new("replay"),
                        ),
                    ),
                ),
            ))
            .unwrap();
        }
        assert_eq!(drain_replay_notifications(&rx), 3);
        assert!(
            rx.try_recv().is_err(),
            "channel should be empty after draining"
        );
    }

    #[test]
    fn drain_replay_notifications_is_a_no_op_on_an_empty_channel() {
        let (_tx, rx) = smol::channel::unbounded::<SessionNotification>();
        assert_eq!(drain_replay_notifications(&rx), 0);
    }
}
