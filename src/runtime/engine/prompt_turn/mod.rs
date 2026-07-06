//! Ports `others/acpx/src/runtime/engine/prompt-turn.ts` plus the
//! turn-execution half of `manager.ts`'s `startTurn`: runs exactly one
//! turn given "permission already granted". **Deliberately queue-agnostic**
//! per Requirement 6 — this module does not check "is another turn already
//! running on this session"; that single-flight gate is Phase 6's
//! per-session queue (ADR-4), layered *around* [`start_turn`], not inside
//! it.
//!
//! ## Turn lifecycle
//!
//! 1. Build the ACP `ContentBlock`s from the turn input, record the user
//!    message into the session's in-memory conversation model.
//! 2. Spawn a background task ([`task::run_turn_task`]) that: (a) drains
//!    the connected session's raw `session/update` notification feed for
//!    the lifetime of the turn, forwarding parsed [`AcpRuntimeEvent`]s to
//!    the caller's stream and folding each update into the conversation
//!    model (via [`update_mapping`]); (b) sends `session/prompt` and races
//!    it against an optional cancel signal — cancelling sends
//!    `session/cancel` and then keeps awaiting the agent's (now-cancelled)
//!    response, per ACP's cooperative-cancellation contract, rather than
//!    abandoning the request.
//! 3. On completion (success, cancellation, or failure — mapped to a
//!    result by [`turn_result`]), merge the conversation model back into
//!    the record and persist it.
//!
//! Token-usage breakdowns from `UsageUpdate._meta.usage` are surfaced live
//! (see `runtime::public::events`) but not additionally persisted into
//! `request_token_usage` here — this build has no `PromptResponse.usage`
//! field (the `unstable_end_turn_token_usage` schema feature isn't
//! enabled), so there is no terminal-response usage payload to record via
//! [`crate::session::conversation_model::record_prompt_response_usage`];
//! only the live `cumulative_cost` from `UsageUpdate.cost` is folded in.
//!
//! Split (per the workspace's <200-line file guideline) into this module
//! (the [`start_turn`] entry point: setup, then hands off to a detached
//! task), [`task`] (the detached task's actual execution), [`turn_result`]
//! (outcome -> [`AcpRuntimeTurnResult`] mapping), and [`update_mapping`]
//! (live `SessionUpdate` -> persisted-history delta mapping).

mod task;
mod turn_result;
mod update_mapping;

use std::sync::Arc;
use std::time::Duration;

use futures::channel::oneshot;
use futures::stream::BoxStream;

use crate::runtime::engine::connected_session::ConnectedSession;
use crate::runtime::public::contract::{
    AcpRuntimeTurn, AcpRuntimeTurnInput, AcpRuntimeTurnResult, AcpRuntimeTurnResultError,
    AcpSessionStore, attachment_content_blocks,
};
use crate::runtime::public::errors::{AcpRuntimeError, AcpRuntimeErrorCode};
use crate::runtime::public::events::AcpRuntimeEvent;
use crate::session::conversation_model::agent_content::InboundContent;
use crate::session::conversation_model::record::record_prompt_submission;

use task::run_turn_task;

const EVENT_CHANNEL_CAPACITY: usize = 512;

/// Ports `startTurn`'s turn-execution body (queue-agnostic, see module
/// docs). Returns immediately with a live [`AcpRuntimeTurn`]; the actual
/// RPC work runs on a detached `smol` task ([`task::run_turn_task`]).
///
/// `on_slot_freed` is Phase 6's hook (`crate::queue::dispatcher`): fired
/// exactly once, right when this turn's slot becomes reusable (success,
/// RPC error, timeout, or cancellation all count — same "done" moment as
/// `connected.set_active_prompt(false)` below), so the per-session queue
/// knows when to dispatch the next pending request. `None` when called
/// outside the queue (there is currently no such caller, but the parameter
/// stays optional so this function doesn't *require* a queue to exist).
pub fn start_turn(
    connected: Arc<ConnectedSession>,
    session_store: Arc<dyn AcpSessionStore>,
    input: AcpRuntimeTurnInput,
    default_timeout_ms: Option<u64>,
    on_slot_freed: Option<oneshot::Sender<()>>,
) -> Result<AcpRuntimeTurn, AcpRuntimeError> {
    let content_blocks = attachment_content_blocks(&input.text, &input.attachments)
        .map_err(|err| AcpRuntimeError::new(AcpRuntimeErrorCode::TurnFailed, err.message))?;
    let request_id = input.request_id.clone();
    let timeout = input
        .timeout_ms
        .or(default_timeout_ms)
        .map(Duration::from_millis);

    // Gap 6: capture the recorded prompt message id (previously discarded)
    // so the turn task can check `has_agent_reply_after_prompt` on a timeout.
    let prompt_message_id = {
        let mut conversation = connected.conversation.lock();
        record_prompt_submission(
            &mut conversation,
            &[InboundContent::Text(input.text.clone())],
            None,
        )
    };

    let (event_tx, event_rx) = smol::channel::bounded::<AcpRuntimeEvent>(EVENT_CHANNEL_CAPACITY);
    let (cancel_tx, cancel_rx) = oneshot::channel::<Option<String>>();
    let (result_tx, result_rx) = oneshot::channel::<AcpRuntimeTurnResult>();

    let session_id = connected.session_id();
    connected.set_active_prompt(true);

    smol::spawn(run_turn_task(
        connected,
        session_store,
        session_id,
        content_blocks,
        timeout,
        prompt_message_id,
        event_tx,
        cancel_rx,
        result_tx,
        on_slot_freed,
    ))
    .detach();

    let events: BoxStream<'static, AcpRuntimeEvent> = Box::pin(event_rx);
    let result = Box::pin(async move {
        result_rx.await.unwrap_or(AcpRuntimeTurnResult::Failed {
            error: AcpRuntimeTurnResultError {
                message: "turn task ended without reporting a result".to_string(),
                code: None,
                detail_code: None,
                retryable: Some(false),
            },
        })
    });
    Ok(AcpRuntimeTurn::new(request_id, events, result, cancel_tx))
}
