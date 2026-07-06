//! Archive-shaping helpers: event-log history collection, liveness
//! checking, and cwd/event-log redaction for the archived record state.
//!
//! Ports the remaining helpers of `others/acpx/src/session/export.ts`:
//! `readSessionHistory`, `isSessionActive`/`hasLiveEventLock`,
//! `cwdRelativeToHome`, `serializeSessionRecordForArchive`.

use std::path::Path;

use serde_json::Value;

use crate::session::event_log::{
    session_event_active_path, session_event_lock_path, session_event_segment_path,
};
use crate::session::persistence::serialize::serialize_session_record_for_disk;
use crate::session::record::SessionRecord;
use crate::session::store_options::AcpFileSessionStoreOptions;

fn read_history_file(path: &str) -> Vec<Value> {
    let Ok(payload) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    payload
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Ports `readSessionHistory`.
pub(super) fn read_session_history(
    options: &AcpFileSessionStoreOptions,
    record: &SessionRecord,
) -> Vec<Value> {
    let mut history = Vec::new();
    let max_segments = record.event_log.max_segments;
    for segment in (1..=max_segments).rev() {
        history.extend(read_history_file(&session_event_segment_path(
            options,
            &record.acpx_record_id,
            segment,
        )));
    }
    history.extend(read_history_file(&session_event_active_path(
        options,
        &record.acpx_record_id,
    )));
    history
}

fn has_live_event_lock(options: &AcpFileSessionStoreOptions, record_id: &str) -> bool {
    let lock_path = session_event_lock_path(options, record_id);
    let Ok(payload) = std::fs::read_to_string(&lock_path) else {
        return false;
    };
    let pid = serde_json::from_str::<Value>(&payload)
        .ok()
        .and_then(|value| value.get("pid").and_then(Value::as_u64))
        .map(|pid| pid as u32);
    pid.is_some_and(crate::platform::is_process_alive)
}

/// Ports `isSessionActive`.
pub(super) fn is_session_active(
    options: &AcpFileSessionStoreOptions,
    record: &SessionRecord,
) -> bool {
    if record.closed {
        return false;
    }
    record.pid.is_some_and(crate::platform::is_process_alive)
        || has_live_event_lock(options, &record.acpx_record_id)
}

/// Ports `cwdRelativeToHome`.
pub(super) fn cwd_relative_to_home(cwd: &str, home: &Path) -> String {
    let cwd_path = Path::new(cwd);
    match cwd_path.strip_prefix(home) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
        Ok(relative) => relative.to_string_lossy().into_owned(),
        Err(_) => cwd.to_string(),
    }
}

/// Ports `serializeSessionRecordForArchive`.
pub(super) fn serialize_session_record_for_archive(
    record: &SessionRecord,
    cwd_relative: &str,
) -> Value {
    let mut state = serialize_session_record_for_disk(record);
    state["cwd"] = Value::String(cwd_relative.to_string());
    if let Some(event_log) = state.get_mut("event_log").and_then(Value::as_object_mut) {
        event_log.insert(
            "active_path".to_string(),
            Value::String(".stream.ndjson".to_string()),
        );
    }
    state
}
