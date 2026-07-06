//! The detached task [`super::start_turn`] spawns: races `session/prompt`
//! against an optional cancel signal while a sibling task drains live
//! `session/update` notifications, then persists the result. Split out of
//! `prompt_turn/mod.rs` (setup vs. execution) per the workspace's per-file
//! line convention.

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{ContentBlock, PromptRequest, StopReason};
use futures::channel::oneshot;
use futures::future::{Either, select};
use futures::pin_mut;

use crate::control::with_timeout;
use crate::error::AcpError;
use crate::runtime::engine::connected_session::ConnectedSession;
use crate::runtime::engine::lifecycle::apply_conversation;
use crate::runtime::public::contract::{AcpRuntimeTurnResult, AcpSessionStore};
use crate::runtime::public::events::{AcpRuntimeEvent, parse_session_update};

use super::turn_result::{
    turn_result_from_rpc_error, turn_result_from_stop_reason, turn_result_from_timeout,
};
use super::update_mapping::session_update_input;
use crate::session::conversation_model::record::{
    has_agent_reply_after_prompt, record_client_operation,
};
use crate::session::conversation_model::session_update::record_session_update;

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_turn_task(
    connected: Arc<ConnectedSession>,
    session_store: Arc<dyn AcpSessionStore>,
    session_id: agent_client_protocol::schema::v1::SessionId,
    content_blocks: Vec<ContentBlock>,
    timeout: Option<Duration>,
    prompt_message_id: Option<String>,
    event_tx: smol::channel::Sender<AcpRuntimeEvent>,
    cancel_rx: oneshot::Receiver<Option<String>>,
    result_tx: oneshot::Sender<AcpRuntimeTurnResult>,
    on_slot_freed: Option<oneshot::Sender<()>>,
) {
    let drain_task = smol::spawn(drain_notifications(
        connected.clone(),
        session_id.clone(),
        event_tx.clone(),
    ));
    // Gap 20: drain filesystem/terminal client-operation events for the
    // lifetime of the turn, persisting each (`record_client_operation`) and
    // streaming it as an `AcpRuntimeEvent::ClientOperation`.
    let operation_drain_task = smol::spawn(drain_operations(connected.clone(), event_tx.clone()));

    let request = PromptRequest::new(session_id.clone(), content_blocks);
    let prompt_future = connected
        .client
        .connection()
        .send_request(request)
        .block_task();
    pin_mut!(prompt_future);

    // Cloning the `Arc` (not `AcpClient` itself, which isn't `Clone`) so the
    // `async move` block below can own a handle to send `session/cancel`
    // without taking `connected` away from the rest of this function.
    let race_connected = connected.clone();
    let race_session_id = session_id.clone();
    let raced = async move {
        match select(prompt_future.as_mut(), cancel_rx).await {
            Either::Left((response, _cancel_rx)) => response,
            Either::Right((_cancel_result, _prompt_future)) => {
                let _ = race_connected.client.cancel_session(race_session_id);
                prompt_future.await
            }
        }
    };

    // Gap 6: remember whether the failure was specifically a prompt timeout,
    // so that after the drain task stops we can check if the agent had in
    // fact already replied (via session/update) before the deadline — in
    // which case acpx reports a normal `end_turn`, not a hard timeout.
    let mut timed_out = false;
    let outcome = match timeout {
        Some(timeout) => match with_timeout(raced, Some(timeout)).await {
            Ok(result) => result.map_err(turn_result_from_rpc_error),
            Err(AcpError::Timeout(d)) => {
                timed_out = true;
                Err(turn_result_from_timeout(d))
            }
            Err(_) => {
                timed_out = true;
                Err(turn_result_from_timeout(timeout))
            }
        },
        None => raced.await.map_err(turn_result_from_rpc_error),
    };

    // Stop draining live updates before persisting: `drain_task` holds the
    // same `record`/`conversation` locks this function is about to take.
    drain_task.cancel().await;
    operation_drain_task.cancel().await;

    connected.set_active_prompt(false);
    if let Some(tx) = on_slot_freed {
        // Best-effort: the queue dispatcher always awaits this (see
        // `crate::queue::dispatcher::dispatch`), but a send failure here
        // (nobody listening) must never fail the turn itself.
        let _ = tx.send(());
    }

    let result = match outcome {
        Ok(response) => turn_result_from_stop_reason(response.stop_reason),
        // Gap 6: a prompt RPC timeout is not a hard failure if the agent
        // actually replied before the deadline (`hasAgentReplyAfterPrompt`).
        // The drain task (now stopped) folded any in-flight `session/update`s
        // into the conversation, so this reflects what the agent really sent.
        Err(_timeout_result)
            if timed_out
                && prompt_message_id.as_deref().is_some_and(|id| {
                    has_agent_reply_after_prompt(&connected.conversation.lock(), id)
                }) =>
        {
            turn_result_from_stop_reason(StopReason::EndTurn)
        }
        Err(result) => result,
    };

    {
        let mut record = connected.record.lock();
        let conversation = connected.conversation.lock();
        apply_conversation(&mut record, &conversation);
        let now = crate::session::conversation_model::conversation::iso_now();
        record.last_prompt_at = Some(now.clone());
        record.last_used_at = now;
    }
    let snapshot = connected.record.lock().clone();
    let _ = session_store.save(snapshot).await;

    let terminal_event = match &result {
        AcpRuntimeTurnResult::Completed { stop_reason }
        | AcpRuntimeTurnResult::Cancelled { stop_reason } => AcpRuntimeEvent::Done {
            stop_reason: stop_reason.clone(),
        },
        AcpRuntimeTurnResult::Failed { error } => AcpRuntimeEvent::Error {
            message: error.message.clone(),
            code: error.code.clone(),
            detail_code: error.detail_code.clone(),
            retryable: error.retryable,
        },
    };
    let _ = event_tx.try_send(terminal_event);
    let _ = result_tx.send(result);
}

async fn drain_notifications(
    connected: Arc<ConnectedSession>,
    session_id: agent_client_protocol::schema::v1::SessionId,
    event_tx: smol::channel::Sender<AcpRuntimeEvent>,
) {
    while let Ok(notification) = connected.notifications.recv().await {
        if notification.session_id != session_id {
            continue;
        }
        // Requirement 4: apply this notification's live-event forward +
        // persisted-history fold under the session's update-ordering lock,
        // making "processed in arrival order" an explicit invariant rather
        // than an incidental consequence of this being a single-consumer
        // loop. See `ConnectedSession::update_order`'s doc comment for why
        // this must stay a separate lock from the prompt queue's.
        connected.with_ordered_update(|| {
            if let Some(event) = parse_session_update(&notification.update) {
                let _ = event_tx.try_send(event);
            }
            if let Some(update_input) = session_update_input(&notification.update) {
                let mut conversation = connected.conversation.lock();
                let mut record = connected.record.lock();
                let acpx = record.acpx.get_or_insert_with(Default::default);
                record_session_update(&mut conversation, acpx, update_input, None);
            }
        });
        // Gap 16: count each live-consumed update (bumps both observed and
        // processed), so `session_update_counts()`'s observed-vs-processed
        // gap actually reflects the load-time suppressed-replay count rather
        // than being a permanently-`0` diagnostic.
        connected.record_processed_update();
    }
}

/// Gap 20: forwards the connected session's filesystem/terminal
/// client-operation events (fed by their `on_operation` callbacks, see
/// `manager_spawn.rs`) into the turn — persisting each via
/// `record_client_operation` under the session update-ordering lock and
/// streaming it as an [`AcpRuntimeEvent::ClientOperation`]. Mirrors acpx's
/// `manager.ts` dual persisted+streamed `onClientOperation` handling.
async fn drain_operations(
    connected: Arc<ConnectedSession>,
    event_tx: smol::channel::Sender<AcpRuntimeEvent>,
) {
    while let Ok(op) = connected.operations.recv().await {
        connected.with_ordered_update(|| {
            let mut conversation = connected.conversation.lock();
            record_client_operation(&mut conversation, Some(op.timestamp.clone()));
        });
        let _ = event_tx.try_send(AcpRuntimeEvent::ClientOperation {
            method: op.method,
            status: op.status,
            summary: op.summary,
            details: op.details,
            timestamp: op.timestamp,
        });
    }
}
