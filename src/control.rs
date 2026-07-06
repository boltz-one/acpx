//! Timeout helper ported from `others/acpx/src/async-control.ts`.
//!
//! `withInterrupt` (SIGINT/SIGTERM/SIGHUP handling for the interactive CLI)
//! is intentionally not ported — it is a terminal/CLI concern outside this
//! crate's scope; cooperative cancellation for an embedded GUI runtime is
//! addressed in Phase 6 (in-process queueing) instead.

use std::future::Future;
use std::time::Duration;

use futures::future::{Either, select};
use futures::pin_mut;
use smol::Timer;

use crate::error::{AcpError, Result};

/// Race `fut` against a `timeout`. Ports `withTimeout` from
/// `async-control.ts`: `None` (or a zero/negative duration) disables the
/// timeout and simply awaits `fut`, matching acpx's `timeoutMs == null`
/// short-circuit.
pub async fn with_timeout<F: Future>(fut: F, timeout: Option<Duration>) -> Result<F::Output> {
    let Some(timeout) = timeout.filter(|d| !d.is_zero()) else {
        return Ok(fut.await);
    };

    pin_mut!(fut);
    let timer = Timer::after(timeout);
    match select(fut, timer).await {
        Either::Left((value, _)) => Ok(value),
        Either::Right((_, _)) => Err(AcpError::Timeout(timeout)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_timeout_awaits_plain_future() {
        smol::block_on(async {
            let result = with_timeout(async { 42 }, None).await.unwrap();
            assert_eq!(result, 42);
        });
    }

    #[test]
    fn expired_timeout_yields_timeout_error() {
        smol::block_on(async {
            let result = with_timeout(
                async {
                    Timer::after(Duration::from_secs(3600)).await;
                },
                Some(Duration::from_millis(1)),
            )
            .await;
            assert!(matches!(result, Err(AcpError::Timeout(_))));
        });
    }
}
