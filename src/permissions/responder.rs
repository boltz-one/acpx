//! ADR-6: async/channel-based permission-request handling. Replaces
//! `others/acpx/src/permission-prompt.ts`'s synchronous stdin TTY prompt
//! entirely — a GPUI app has no stdin TTY, and blocking a `smol` worker
//! thread on user interaction could stall unrelated sessions sharing that
//! worker (see the phase's ADR-6 rationale for the full alternatives
//! analysis).
//!
//! [`PermissionRequestHandler`] is the injected async callback; whenever
//! acpx's `canPromptForPermission()` branch would read stdin, this crate
//! instead `.await`s `handler.request(...)` — an ordinary future, so it
//! only suspends the in-flight RPC response, never the caller's executor.
//! [`ChannelPermissionRequestHandler`] is a ready-to-use, GPUI-agnostic
//! implementation: it forwards each request plus a paired [`PermissionResponder`]
//! over an (unbounded) channel to whatever owns the UI, which eventually
//! calls [`PermissionResponder::respond`].

use agent_client_protocol::schema::v1::RequestPermissionRequest;
use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::BoxFuture;

/// The user's (or caller's) decision on an interactive permission request.
/// Ports acpx's `AcpPermissionDecision` (`{ outcome: "allow_once" | ... }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Cancel,
}

/// Injected async permission-decision source. The GPUI app implements this
/// (or uses [`ChannelPermissionRequestHandler`]) to show a dialog and
/// resolve the returned future once the user responds; a fake/delayed
/// implementation is used in tests to prove the wait doesn't block other
/// concurrent work (see `permissions::tests`).
pub trait PermissionRequestHandler: Send + Sync {
    fn request(&self, params: RequestPermissionRequest) -> BoxFuture<'static, PermissionDecision>;
}

/// One-shot handle for resolving a single permission request dispatched by
/// [`ChannelPermissionRequestHandler`]. Dropping it without calling
/// [`Self::respond`] resolves the paired future to
/// [`PermissionDecision::Cancel`].
pub struct PermissionResponder(oneshot::Sender<PermissionDecision>);

impl PermissionResponder {
    pub fn respond(self, decision: PermissionDecision) {
        // The receiver may already be gone if the RPC that asked was itself
        // cancelled; that's not this responder's problem to report.
        let _ = self.0.send(decision);
    }
}

/// One permission request forwarded to whatever consumes
/// [`ChannelPermissionRequestHandler`]'s receiver (e.g. the GPUI app's
/// session-event loop).
pub struct PermissionRequestEnvelope {
    pub request: RequestPermissionRequest,
    pub responder: PermissionResponder,
}

/// A [`PermissionRequestHandler`] that forwards every request over an
/// `smol::channel` to a consumer, and awaits a paired oneshot for the
/// decision. Generic over the eventual consumer (GPUI dialog, test harness,
/// …) so this crate doesn't need a GPUI dependency to satisfy ADR-6.
#[derive(Clone)]
pub struct ChannelPermissionRequestHandler {
    outbox: smol::channel::Sender<PermissionRequestEnvelope>,
}

impl ChannelPermissionRequestHandler {
    /// Builds a handler plus the receiver its consumer polls for incoming
    /// requests.
    pub fn new() -> (Self, smol::channel::Receiver<PermissionRequestEnvelope>) {
        let (outbox, inbox) = smol::channel::unbounded();
        (Self { outbox }, inbox)
    }
}

impl PermissionRequestHandler for ChannelPermissionRequestHandler {
    fn request(&self, params: RequestPermissionRequest) -> BoxFuture<'static, PermissionDecision> {
        let outbox = self.outbox.clone();
        async move {
            let (tx, rx) = oneshot::channel();
            let envelope = PermissionRequestEnvelope {
                request: params,
                responder: PermissionResponder(tx),
            };
            if outbox.send(envelope).await.is_err() {
                // No consumer is listening (e.g. the UI already shut down);
                // treat like the user cancelled rather than hanging forever.
                return PermissionDecision::Cancel;
            }
            rx.await.unwrap_or(PermissionDecision::Cancel)
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{ToolCallId, ToolCallUpdate, ToolCallUpdateFields};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fake_request() -> RequestPermissionRequest {
        RequestPermissionRequest::new(
            "session-1",
            ToolCallUpdate::new(ToolCallId::new("tool-1"), ToolCallUpdateFields::new()),
            vec![],
        )
    }

    #[test]
    fn channel_handler_delivers_request_and_returns_decision() {
        smol::block_on(async {
            let (handler, inbox) = ChannelPermissionRequestHandler::new();

            let consumer = smol::spawn(async move {
                let envelope = inbox.recv().await.expect("request delivered");
                envelope.responder.respond(PermissionDecision::AllowOnce);
            });

            let decision = handler.request(fake_request()).await;
            assert_eq!(decision, PermissionDecision::AllowOnce);
            consumer.await;
        });
    }

    #[test]
    fn dropped_responder_resolves_to_cancel() {
        smol::block_on(async {
            let (handler, inbox) = ChannelPermissionRequestHandler::new();
            let consumer = smol::spawn(async move {
                let envelope = inbox.recv().await.expect("request delivered");
                drop(envelope.responder);
            });

            let decision = handler.request(fake_request()).await;
            assert_eq!(decision, PermissionDecision::Cancel);
            consumer.await;
        });
    }

    /// Success criterion: a delayed permission decision must not block
    /// unrelated concurrent work on the same executor. A background task
    /// increments a counter on a tight loop while the permission request is
    /// pending; if awaiting the handler blocked the executor thread, the
    /// counter would never advance before the decision arrives.
    #[test]
    fn pending_request_does_not_block_other_concurrent_work() {
        smol::block_on(async {
            let (handler, inbox) = ChannelPermissionRequestHandler::new();
            let counter = Arc::new(AtomicUsize::new(0));

            let ticker = smol::spawn({
                let counter = counter.clone();
                async move {
                    for _ in 0..5 {
                        smol::Timer::after(std::time::Duration::from_millis(5)).await;
                        counter.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });

            let responder_task = smol::spawn(async move {
                let envelope = inbox.recv().await.expect("request delivered");
                smol::Timer::after(std::time::Duration::from_millis(50)).await;
                envelope.responder.respond(PermissionDecision::RejectOnce);
            });

            let decision = handler.request(fake_request()).await;
            assert_eq!(decision, PermissionDecision::RejectOnce);
            ticker.await;
            responder_task.await;
            assert!(counter.load(Ordering::SeqCst) >= 3);
        });
    }
}
