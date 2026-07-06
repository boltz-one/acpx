//! JSON-lines perf metrics capture to a file, armed by an env var or an
//! explicit path. Ports `others/acpx/src/perf-metrics-capture.ts`.
//!
//! **Deviation from the TS source:** acpx installs `process.once("exit", ...)`
//! plus `SIGINT`/`SIGTERM` handlers so a short-lived CLI process auto-flushes
//! on shutdown. This crate is embedded in a long-running GPUI desktop app
//! that already owns its own process/signal lifecycle (see `crate::control`,
//! `crate::client::shutdown`) — installing a second, competing global
//! `SIGINT`/`SIGTERM` handler here would fight with that. So
//! [`install_perf_metrics_capture`] only ports the config/state half of
//! `installPerfMetricsCapture` (arming the capture file path, resetting
//! metrics, seeding role/argv/sequence); the embedding app is expected to
//! call [`flush_perf_metrics_capture`] explicitly from its own shutdown and
//! signal-handling paths instead. `checkpointPerfMetricsCapture`'s and
//! `flushPerfMetricsCapture`'s actual write logic is ported as-is.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde::Serialize;

use super::{PerfMetricsSnapshot, get_perf_metrics_snapshot, reset_perf_metrics};

/// Ports `PERF_METRICS_FILE_ENV`. Renamed from acpx's `ACPX_PERF_METRICS_FILE`
/// to match this crate's identity.
const PERF_METRICS_FILE_ENV: &str = "BOLTZ_ACP_PERF_METRICS_FILE";

/// Ports `CaptureReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureReason {
    Checkpoint,
    Exit,
    Signal,
}

struct CaptureState {
    file_path: Option<String>,
    role: String,
    argv: Vec<String>,
    sequence: u64,
    flushed: bool,
}

impl Default for CaptureState {
    fn default() -> Self {
        Self {
            file_path: None,
            role: "cli".to_string(),
            argv: Vec::new(),
            sequence: 0,
            flushed: false,
        }
    }
}

fn state() -> &'static Mutex<CaptureState> {
    static STATE: OnceLock<Mutex<CaptureState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(CaptureState::default()))
}

/// Ports `shouldCapture`.
fn should_capture(state: &CaptureState) -> bool {
    state
        .file_path
        .as_deref()
        .is_some_and(|path| !path.trim().is_empty())
}

#[derive(Debug, Serialize)]
struct CapturePayload {
    timestamp: String,
    pid: u32,
    ppid: u32,
    role: String,
    argv: Vec<String>,
    cwd: String,
    sequence: u64,
    reason: CaptureReason,
    metrics: PerfMetricsSnapshot,
}

/// `process.ppid` has no portable stdlib equivalent; `libc::getppid` covers
/// unix (this crate already depends on `libc` there for signal handling —
/// see `crate::platform::liveness`). No `windows` crate API is wired for
/// this optional profiling field, so it's `0` there.
#[cfg(unix)]
fn parent_process_id() -> u32 {
    unsafe { libc::getppid() as u32 }
}

#[cfg(not(unix))]
fn parent_process_id() -> u32 {
    0
}

/// Ports `buildPayload`.
fn build_payload(state: &CaptureState, reason: CaptureReason) -> CapturePayload {
    CapturePayload {
        timestamp: crate::session::conversation_model::iso_now(),
        pid: std::process::id(),
        ppid: parent_process_id(),
        role: state.role.clone(),
        argv: state.argv.clone(),
        cwd: std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        sequence: state.sequence,
        reason,
        metrics: get_perf_metrics_snapshot(),
    }
}

/// Ports `appendPerfMetricsPayload` (mkdir -p the parent dir, then append a
/// single JSON line).
fn append_perf_metrics_payload(file_path: &str, payload: &CapturePayload) -> std::io::Result<()> {
    if let Some(parent) = Path::new(file_path).parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(payload)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path)?;
    writeln!(file, "{line}")
}

/// Ports `writePerfMetricsCapture`. Best-effort: any I/O error is swallowed
/// and reported as `false`, matching the TS `catch { return false; }`
/// ("metrics capture is best-effort only").
fn write_perf_metrics_capture(reason: CaptureReason, reset_after_write: bool) -> bool {
    let mut guard = state().lock();
    if !should_capture(&guard) {
        return false;
    }

    let payload = build_payload(&guard, reason);
    if payload.metrics.is_empty() {
        return false;
    }

    let file_path = guard
        .file_path
        .clone()
        .expect("should_capture just checked this is Some");
    let wrote = append_perf_metrics_payload(&file_path, &payload).is_ok();
    if wrote {
        guard.sequence += 1;
    }
    drop(guard);

    if wrote && reset_after_write {
        reset_perf_metrics();
    }
    wrote
}

/// Ports `checkpointPerfMetricsCapture`.
pub fn checkpoint_perf_metrics_capture() {
    state().lock().flushed = false;
    write_perf_metrics_capture(CaptureReason::Checkpoint, true);
}

/// Ports `flushPerfMetricsCapture`. The TS source defaults `reason` to
/// `"exit"`; pass [`CaptureReason::Exit`] explicitly to match that call
/// shape (Rust has no default parameters).
pub fn flush_perf_metrics_capture(reason: CaptureReason) {
    let mut guard = state().lock();
    if guard.flushed || !should_capture(&guard) {
        return;
    }
    guard.flushed = true;
    drop(guard);
    write_perf_metrics_capture(reason, false);
}

/// Options for [`install_perf_metrics_capture`]. Ports `installPerfMetricsCapture`'s
/// options bag.
#[derive(Debug, Default, Clone)]
pub struct PerfMetricsCaptureOptions {
    pub argv: Vec<String>,
    pub role: Option<String>,
    pub file_path: Option<String>,
}

/// Ports `installPerfMetricsCapture`'s config/state half — see this module's
/// doc comment for why the `process.once("exit"/"SIGINT"/"SIGTERM")` hook
/// installation is intentionally not ported.
pub fn install_perf_metrics_capture(options: PerfMetricsCaptureOptions) {
    let mut guard = state().lock();
    guard.file_path = options
        .file_path
        .or_else(|| std::env::var(PERF_METRICS_FILE_ENV).ok());
    if !should_capture(&guard) {
        return;
    }
    drop(guard);

    reset_perf_metrics();

    let mut guard = state().lock();
    if let Some(role) = options.role {
        guard.role = role;
    }
    guard.argv = options.argv;
    guard.sequence = 0;
    guard.flushed = false;
}

/// Ports `perfMetricsCaptureFileFromEnv`, reading the real process
/// environment.
pub fn perf_metrics_capture_file_from_env() -> Option<String> {
    perf_metrics_capture_file_from_env_map(&std::env::vars().collect())
}

/// Ports `perfMetricsCaptureFileFromEnv(env)`'s injectable-map overload —
/// split out so tests don't need to mutate the real process environment
/// (unlike Node, `std::env::set_var` is `unsafe` and process-global in
/// current Rust, so it's a poor fit for parallel unit tests).
fn perf_metrics_capture_file_from_env_map(env: &HashMap<String, String>) -> Option<String> {
    env.get(PERF_METRICS_FILE_ENV)
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

// Split out per the workspace's <200-line file guideline; logically still
// part of this module (`super::*` sees its private items).
#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
