//! Per-session FIFO + single-flight slot state machine backing
//! [`super::SessionPromptQueue`]. Pure, `async`-free: state transitions are
//! plain function calls guarded by one [`parking_lot::Mutex`], so this
//! module is unit-tested here without spinning up `smol` or any real prompt
//! machinery.
//!
//! ## State machine (Requirement 1-3, Step 2 — designed before coding)
//!
//! ```text
//!               try_admit(req)                 free_slot()
//!                 [Idle]  --------------->  [Running]
//!                   ^                           |  ^
//!                   |     free_slot(), no more  |  | try_admit(req) while
//!                   +---------- pending ---------+  | Running (capacity
//!                                                    | available)
//!                                                    v
//!                                          pending: VecDeque<T> (FIFO)
//! ```
//!
//! - `Idle -> Running`: [`SessionQueueState::try_admit`] finds the slot
//!   empty, flips it to `Running`, and hands the payload straight back
//!   (`Admission::RunNow`) so the caller dispatches it immediately.
//! - `Running`, capacity available: the payload is pushed onto the bounded
//!   `VecDeque` (`Admission::Queued`); the caller does not dispatch it —
//!   [`SessionQueueState::free_slot`] will, later.
//! - `Running`, capacity exhausted: `Admission::Rejected(payload)` hands the
//!   payload straight back **unqueued** so the caller can report bounded
//!   backpressure (Requirement 1) without blocking or silently dropping it.
//! - `Running -> Idle` (or `Running -> Running`): [`SessionQueueState::free_slot`]
//!   is called exactly once per admitted payload's completion (success,
//!   error, or cancellation all count, per Step 4). It pops the FIFO front,
//!   if any, and keeps the slot `Running` for that payload (submission
//!   order preserved — Success Criteria #2); otherwise the slot returns to
//!   `Idle`.
//!
//! ## Locking invariant
//!
//! The one `Mutex` this module defines guards *only* admission bookkeeping
//! (which slot a payload is in) — it is held for the duration of a plain
//! data-structure mutation, never across an `.await`. It must never be
//! acquired while holding `ConnectedSession`'s `update_order` lock (session/
//! update ordering, Requirement 4) or vice versa; see that field's doc
//! comment. This module has no knowledge of `update_order` at all, which is
//! what makes the invariant true by construction rather than by discipline.

use std::collections::VecDeque;

use parking_lot::Mutex;

enum SlotState {
    Idle,
    Running,
}

struct QueueState<T> {
    slot: SlotState,
    pending: VecDeque<T>,
    capacity: usize,
}

/// Result of [`SessionQueueState::try_admit`].
pub(super) enum Admission<T> {
    /// The slot was idle; the payload is now the running one — dispatch it.
    RunNow(T),
    /// The slot was running and there was room in the bounded FIFO; the
    /// payload is queued. [`SessionQueueState::free_slot`] will surface it
    /// later.
    Queued,
    /// The bounded FIFO was already at capacity; the payload is handed back
    /// unqueued so the caller can report backpressure.
    Rejected(T),
}

/// The state machine described in this module's docs, generic over the
/// payload type so it stays testable without real prompt/turn machinery.
pub(super) struct SessionQueueState<T> {
    inner: Mutex<QueueState<T>>,
}

impl<T> SessionQueueState<T> {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(QueueState {
                slot: SlotState::Idle,
                pending: VecDeque::new(),
                capacity,
            }),
        }
    }

    pub(super) fn capacity(&self) -> usize {
        self.inner.lock().capacity
    }

    pub(super) fn pending_len(&self) -> usize {
        self.inner.lock().pending.len()
    }

    pub(super) fn try_admit(&self, payload: T) -> Admission<T> {
        let mut state = self.inner.lock();
        match state.slot {
            SlotState::Idle => {
                state.slot = SlotState::Running;
                Admission::RunNow(payload)
            }
            SlotState::Running => {
                if state.pending.len() >= state.capacity {
                    Admission::Rejected(payload)
                } else {
                    state.pending.push_back(payload);
                    Admission::Queued
                }
            }
        }
    }

    /// The running payload just finished (success, error, or cancellation —
    /// Step 4). Pops the FIFO front and keeps the slot `Running` for it
    /// (returned so the caller dispatches it), or returns to `Idle`.
    pub(super) fn free_slot(&self) -> Option<T> {
        let mut state = self.inner.lock();
        match state.pending.pop_front() {
            Some(next) => {
                state.slot = SlotState::Running;
                Some(next)
            }
            None => {
                state.slot = SlotState::Idle;
                None
            }
        }
    }

    /// Drops every queued-but-not-started payload, leaving the running slot
    /// (if any) untouched — Requirement 3/Step 6's "cancel does not clear
    /// the queue by default" split: this is the explicit opt-in half.
    pub(super) fn clear_pending(&self) -> Vec<T> {
        let mut state = self.inner.lock();
        state.pending.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_admits_immediately() {
        let q: SessionQueueState<u32> = SessionQueueState::new(2);
        assert!(matches!(q.try_admit(1), Admission::RunNow(1)));
    }

    #[test]
    fn running_queues_up_to_capacity_then_rejects() {
        let q: SessionQueueState<u32> = SessionQueueState::new(2);
        assert!(matches!(q.try_admit(1), Admission::RunNow(1)));
        assert!(matches!(q.try_admit(2), Admission::Queued));
        assert!(matches!(q.try_admit(3), Admission::Queued));
        match q.try_admit(4) {
            Admission::Rejected(4) => {}
            _ => panic!("expected Rejected(4) once the bound is exceeded"),
        }
        assert_eq!(q.pending_len(), 2);
    }

    #[test]
    fn free_slot_pops_fifo_order_and_returns_to_idle_when_empty() {
        let q: SessionQueueState<u32> = SessionQueueState::new(4);
        let _ = q.try_admit(1);
        let _ = q.try_admit(2);
        let _ = q.try_admit(3);
        assert_eq!(q.free_slot(), Some(2), "FIFO: first queued item runs next");
        assert_eq!(q.free_slot(), Some(3));
        assert_eq!(q.free_slot(), None, "no more pending -> back to Idle");
        // Idle again: a fresh payload is admitted immediately, not queued.
        assert!(matches!(q.try_admit(4), Admission::RunNow(4)));
    }

    #[test]
    fn clear_pending_drains_queue_without_touching_running_slot() {
        let q: SessionQueueState<u32> = SessionQueueState::new(4);
        let _ = q.try_admit(1); // running
        let _ = q.try_admit(2); // queued
        let _ = q.try_admit(3); // queued
        let cleared = q.clear_pending();
        assert_eq!(cleared, vec![2, 3]);
        assert_eq!(q.pending_len(), 0);
        // Running slot (payload 1) is untouched: free_slot still reports
        // "no more pending" rather than re-admitting anything.
        assert_eq!(q.free_slot(), None);
    }
}
