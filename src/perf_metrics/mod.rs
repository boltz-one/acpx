//! In-process counters/timings/gauges, feature-gated behind Cargo feature
//! `perf-metrics` (off by default — a profiling aid, not required for
//! correctness; deferred stretch goal from phase-06's Implementation Step 8).
//!
//! Ports `others/acpx/src/perf-metrics.ts`. `others/acpx/src/perf-metrics-capture.ts`
//! (the "write a JSON-lines snapshot to a file on checkpoint/exit/signal"
//! half) is ported separately in [`capture`], matching acpx's own file
//! split and this crate's per-file line-count convention.
//!
//! State is process-global (a `static`, like acpx's module-level `Map`s)
//! rather than threaded through `AcpRuntime`/`ConnectedSession`, matching
//! the TS source: these are cross-cutting counters callers reach for from
//! anywhere (e.g. `runtime/engine/reconnect.ts`'s
//! `incrementPerfCounter("runtime.connect_and_load.reused_session")`), not
//! scoped to one session.

mod capture;

pub use capture::{
    CaptureReason, PerfMetricsCaptureOptions, checkpoint_perf_metrics_capture,
    flush_perf_metrics_capture, install_perf_metrics_capture, perf_metrics_capture_file_from_env,
};

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;
use serde::Serialize;

/// Ports `TimingBucket`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct TimingBucket {
    count: u64,
    total_ms: f64,
    max_ms: f64,
}

#[derive(Debug, Default)]
struct PerfMetricsState {
    counters: BTreeMap<String, i64>,
    gauges: BTreeMap<String, f64>,
    timings: BTreeMap<String, TimingBucket>,
}

fn state() -> &'static Mutex<PerfMetricsState> {
    static STATE: OnceLock<Mutex<PerfMetricsState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(PerfMetricsState::default()))
}

/// Test-only lock serializing access to the process-global counters/gauges/
/// timings (and, transitively via [`capture`], the capture config) across
/// this module's and [`capture`]'s unit tests — otherwise cargo's default
/// parallel test execution would race on the same `static` state. Private:
/// descendant modules (`capture` and its `tests`) can still reach it.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Ports `roundMetric` (`Number(value.toFixed(3))`).
fn round_metric(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Ports `incrementPerfCounter`'s default `delta = 1` call shape.
pub fn increment_perf_counter(name: &str) {
    increment_perf_counter_by(name, 1);
}

/// Ports `incrementPerfCounter(name, delta)` for a non-default delta.
pub fn increment_perf_counter_by(name: &str, delta: i64) {
    let mut state = state().lock();
    *state.counters.entry(name.to_string()).or_insert(0) += delta;
}

/// Ports `setPerfGauge`.
pub fn set_perf_gauge(name: &str, value: f64) {
    state().lock().gauges.insert(name.to_string(), value);
}

/// Ports `recordPerfDuration`.
pub fn record_perf_duration(name: &str, duration_ms: f64) {
    let mut state = state().lock();
    let bucket = state.timings.entry(name.to_string()).or_default();
    bucket.count += 1;
    bucket.total_ms += duration_ms;
    bucket.max_ms = bucket.max_ms.max(duration_ms);
}

/// Ports `measurePerf`. Unlike the TS `try/finally`, this records the
/// duration via a drop guard, so it still fires if `run`'s future is
/// dropped (cancelled) before completing — the closest Rust analogue of
/// `finally`'s "runs even when the block doesn't return normally".
pub async fn measure_perf<F, T>(name: &str, run: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let _guard = PerfTimerGuard {
        name: name.to_string(),
        started_at: Instant::now(),
    };
    run.await
}

struct PerfTimerGuard {
    name: String,
    started_at: Instant,
}

impl Drop for PerfTimerGuard {
    fn drop(&mut self) {
        let elapsed_ms = self.started_at.elapsed().as_secs_f64() * 1000.0;
        record_perf_duration(&self.name, elapsed_ms);
    }
}

/// Handle returned by [`start_perf_timer`]. Ports the closure `startPerfTimer`
/// returns in the TS source.
pub struct PerfTimerHandle {
    name: String,
    started_at: Instant,
}

impl PerfTimerHandle {
    /// Records the elapsed time since [`start_perf_timer`] as a duration
    /// sample and returns it in milliseconds. Like the TS closure, calling
    /// this more than once is allowed — each call re-samples elapsed time
    /// from the same start and records another sample.
    pub fn stop(&self) -> f64 {
        let elapsed_ms = self.started_at.elapsed().as_secs_f64() * 1000.0;
        record_perf_duration(&self.name, elapsed_ms);
        elapsed_ms
    }
}

/// Ports `startPerfTimer`.
pub fn start_perf_timer(name: &str) -> PerfTimerHandle {
    PerfTimerHandle {
        name: name.to_string(),
        started_at: Instant::now(),
    }
}

/// Ports `PerfMetricSummary`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PerfMetricSummary {
    pub count: u64,
    #[serde(rename = "totalMs")]
    pub total_ms: f64,
    #[serde(rename = "maxMs")]
    pub max_ms: f64,
}

/// Ports `PerfMetricsSnapshot`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PerfMetricsSnapshot {
    pub counters: BTreeMap<String, i64>,
    pub gauges: BTreeMap<String, f64>,
    pub timings: BTreeMap<String, PerfMetricSummary>,
}

impl PerfMetricsSnapshot {
    /// True if every map is empty. Ports `payloadHasMetrics`'s emptiness
    /// check as a method on the snapshot itself, so [`capture`] can reuse
    /// it directly instead of re-inspecting the wrapping capture payload.
    pub fn is_empty(&self) -> bool {
        self.counters.is_empty() && self.gauges.is_empty() && self.timings.is_empty()
    }
}

/// Ports `getPerfMetricsSnapshot`.
pub fn get_perf_metrics_snapshot() -> PerfMetricsSnapshot {
    let state = state().lock();
    PerfMetricsSnapshot {
        counters: state.counters.clone(),
        gauges: state.gauges.clone(),
        timings: state
            .timings
            .iter()
            .map(|(name, bucket)| {
                (
                    name.clone(),
                    PerfMetricSummary {
                        count: bucket.count,
                        total_ms: round_metric(bucket.total_ms),
                        max_ms: round_metric(bucket.max_ms),
                    },
                )
            })
            .collect(),
    }
}

/// Ports `resetPerfMetrics`.
pub fn reset_perf_metrics() {
    let mut state = state().lock();
    state.counters.clear();
    state.gauges.clear();
    state.timings.clear();
}

/// Ports `formatPerfMetric`.
pub fn format_perf_metric(name: &str, duration_ms: f64) -> String {
    format!("{name}={}ms", round_metric(duration_ms))
}

// Split out per the workspace's <200-line file guideline; logically still
// part of this module (`super::*` sees its private items).
#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
