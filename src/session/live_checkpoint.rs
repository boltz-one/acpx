//! Debounced "save the record" driver.
//!
//! Ports `others/acpx/src/session/live-checkpoint.ts`'s `LiveSessionCheckpoint`
//! class. acpx hand-rolls debounce/coalescing with a dirty flag, a
//! `setTimeout` handle, and a cached in-flight `Promise`; this port gets the
//! same "at most one save in flight, and any `request()`/`checkpoint()`
//! that lands mid-flush triggers exactly one more pass" behavior from a
//! `while (dirty.swap(false)) { save().await }` loop serialized by a
//! `futures::lock::Mutex` (an async mutex naturally coalesces concurrent
//! waiters the way acpx's manual `flushing` promise cache does).
//!
//! Per ADR-2 (`plans/20260705-1718-acpx-to-acp-crate-port/plan.md`), this
//! crate's async substrate is `smol`, so [`LiveSessionCheckpoint::request`]
//! schedules its debounce timer via `smol::spawn(..).detach()`.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::lock::Mutex;

const DEFAULT_LIVE_CHECKPOINT_INTERVAL: Duration = Duration::from_millis(500);

/// Ports `LiveSessionCheckpoint`. Generic over the async `save` closure
/// (acpx's `options.save: () => Promise<void>`).
pub struct LiveSessionCheckpoint<S> {
    save: S,
    interval: Duration,
    dirty: Arc<AtomicBool>,
    flush_lock: Arc<Mutex<()>>,
}

impl<S, Fut> LiveSessionCheckpoint<S>
where
    S: Fn() -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    pub fn new(save: S) -> Self {
        Self::with_interval(save, DEFAULT_LIVE_CHECKPOINT_INTERVAL)
    }

    pub fn with_interval(save: S, interval: Duration) -> Self {
        Self {
            save,
            interval,
            dirty: Arc::new(AtomicBool::new(false)),
            flush_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Ports `request()`: marks dirty and, after `interval`, flushes on a
    /// detached background task. Multiple calls within the same debounce
    /// window collapse into the one pending flush (acpx's `if (this.timer)
    /// return;` guard) — here, that's implicit: an extra `smol::spawn` just
    /// means an extra timer fires and finds `dirty` already cleared by an
    /// earlier one, so it's a no-op pass through the `flush_dirty` loop.
    pub fn request(&self) {
        self.dirty.store(true, Ordering::SeqCst);
        let dirty = self.dirty.clone();
        let flush_lock = self.flush_lock.clone();
        let save = self.save.clone();
        let interval = self.interval;
        smol::spawn(async move {
            smol::Timer::after(interval).await;
            flush_dirty(&dirty, &flush_lock, &save).await;
        })
        .detach();
    }

    /// Ports `checkpoint()`: marks dirty and flushes immediately.
    pub async fn checkpoint(&self) {
        self.dirty.store(true, Ordering::SeqCst);
        self.flush().await;
    }

    /// Ports `flush()`.
    pub async fn flush(&self) {
        flush_dirty(&self.dirty, &self.flush_lock, &self.save).await;
    }
}

async fn flush_dirty<S, Fut>(dirty: &AtomicBool, flush_lock: &Mutex<()>, save: &S)
where
    S: Fn() -> Fut,
    Fut: Future<Output = ()>,
{
    let _guard = flush_lock.lock().await;
    while dirty.swap(false, Ordering::SeqCst) {
        save().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn checkpoint_invokes_save_exactly_once() {
        smol::block_on(async {
            let calls = Arc::new(AtomicUsize::new(0));
            let checkpoint = LiveSessionCheckpoint::new({
                let calls = calls.clone();
                move || {
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });

            checkpoint.checkpoint().await;
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn flush_without_a_pending_request_is_a_no_op() {
        smol::block_on(async {
            let calls = Arc::new(AtomicUsize::new(0));
            let checkpoint = LiveSessionCheckpoint::new({
                let calls = calls.clone();
                move || {
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });

            checkpoint.flush().await;
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        });
    }

    #[test]
    fn request_eventually_flushes_after_the_debounce_interval() {
        smol::block_on(async {
            let calls = Arc::new(AtomicUsize::new(0));
            let checkpoint = LiveSessionCheckpoint::with_interval(
                {
                    let calls = calls.clone();
                    move || {
                        let calls = calls.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                },
                Duration::from_millis(20),
            );

            checkpoint.request();
            smol::Timer::after(Duration::from_millis(200)).await;
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        });
    }
}
