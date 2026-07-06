//! Cross-process advisory lock on a session's event log, with stale-lock
//! recovery.
//!
//! Ports the lock half of `others/acpx/src/session/events.ts`:
//! `acquireEventsLock`/`releaseEventsLock`/`removeStaleEventLock`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::error::{AcpError, Result};
use crate::session::event_log::session_event_lock_path;
use crate::session::store_options::{AcpFileSessionStoreOptions, ensure_session_dir};

use super::now_iso;

const LOCK_RETRY: Duration = Duration::from_millis(15);
const EVENT_LOCK_STALE: Duration = Duration::from_secs(15);

pub(super) struct LockHandle {
    path: PathBuf,
}

fn parse_lock_pid(payload: &str) -> Option<u32> {
    let value: Value = serde_json::from_str(payload).ok()?;
    value.get("pid").and_then(Value::as_u64).map(|v| v as u32)
}

fn parse_lock_created_at(payload: &str) -> Option<SystemTime> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let created_at = value.get("created_at")?.as_str()?;
    let parsed = chrono::DateTime::parse_from_rfc3339(created_at).ok()?;
    Some(UNIX_EPOCH + Duration::from_millis(parsed.timestamp_millis().max(0) as u64))
}

fn remove_stale_event_lock(lock_path: &str) -> bool {
    let Ok(payload) = fs::read_to_string(lock_path) else {
        return true; // already gone
    };
    let pid_alive = parse_lock_pid(&payload).is_some_and(crate::platform::is_process_alive);
    let lock_age = parse_lock_created_at(&payload)
        .and_then(|created| SystemTime::now().duration_since(created).ok())
        .unwrap_or(Duration::MAX);
    if pid_alive && lock_age <= EVENT_LOCK_STALE {
        return false;
    }
    fs::remove_file(lock_path).is_ok()
}

/// Ports `acquireEventsLock`.
pub(super) fn acquire_events_lock(
    options: &AcpFileSessionStoreOptions,
    session_id: &str,
) -> Result<LockHandle> {
    ensure_session_dir(options).map_err(|err| AcpError::Other(err.into()))?;
    let lock_path = session_event_lock_path(options, session_id);
    let payload =
        serde_json::json!({"pid": std::process::id(), "created_at": now_iso()}).to_string();

    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                file.write_all(format!("{payload}\n").as_bytes())
                    .map_err(|err| AcpError::Other(err.into()))?;
                return Ok(LockHandle {
                    path: PathBuf::from(lock_path),
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if remove_stale_event_lock(&lock_path) {
                    continue;
                }
                std::thread::sleep(LOCK_RETRY);
            }
            Err(err) => return Err(AcpError::Other(err.into())),
        }
    }
}

/// Ports `releaseEventsLock`.
pub(super) fn release_events_lock(lock: &LockHandle) {
    let _ = fs::remove_file(&lock.path);
}
