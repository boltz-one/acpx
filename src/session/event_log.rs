//! Session event-log location + default metadata.
//!
//! Ports `others/acpx/src/session/event-log.ts`. The path builders take an
//! [`AcpFileSessionStoreOptions`] instead of acpx's hardcoded
//! `~/.acpx/sessions` (see [`super::store_options`]).

use serde::{Deserialize, Serialize};

use super::store_options::{AcpFileSessionStoreOptions, safe_session_id};

pub const DEFAULT_EVENT_SEGMENT_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_EVENT_MAX_SEGMENTS: u32 = 5;

/// Ports `SessionEventLog` from `types.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEventLog {
    pub active_path: String,
    pub segment_count: u32,
    pub max_segment_bytes: u64,
    pub max_segments: u32,
    #[serde(default)]
    pub last_write_at: Option<String>,
    #[serde(default)]
    pub last_write_error: Option<String>,
}

impl Default for SessionEventLog {
    /// Used only as the `#[serde(default)]` fallback when an on-disk record
    /// is missing `event_log` entirely (backward-compat, phase-05
    /// Requirement #3). Unlike acpx's `defaultSessionEventLog(sessionId)`,
    /// this can't know the owning record's id at `Deserialize` time, so
    /// `active_path` is left empty; callers that need a real path should
    /// call [`default_session_event_log`] with the record id instead.
    fn default() -> Self {
        Self {
            active_path: String::new(),
            segment_count: DEFAULT_EVENT_MAX_SEGMENTS,
            max_segment_bytes: DEFAULT_EVENT_SEGMENT_MAX_BYTES,
            max_segments: DEFAULT_EVENT_MAX_SEGMENTS,
            last_write_at: None,
            last_write_error: None,
        }
    }
}

pub fn session_event_active_path(options: &AcpFileSessionStoreOptions, session_id: &str) -> String {
    options
        .session_dir()
        .join(format!("{}.stream.ndjson", safe_session_id(session_id)))
        .to_string_lossy()
        .into_owned()
}

pub fn session_event_segment_path(
    options: &AcpFileSessionStoreOptions,
    session_id: &str,
    segment: u32,
) -> String {
    options
        .session_dir()
        .join(format!(
            "{}.stream.{segment}.ndjson",
            safe_session_id(session_id)
        ))
        .to_string_lossy()
        .into_owned()
}

pub fn session_event_lock_path(options: &AcpFileSessionStoreOptions, session_id: &str) -> String {
    options
        .session_dir()
        .join(format!("{}.stream.lock", safe_session_id(session_id)))
        .to_string_lossy()
        .into_owned()
}

/// Ports `defaultSessionEventLog`.
pub fn default_session_event_log(
    options: &AcpFileSessionStoreOptions,
    session_id: &str,
) -> SessionEventLog {
    SessionEventLog {
        active_path: session_event_active_path(options, session_id),
        segment_count: DEFAULT_EVENT_MAX_SEGMENTS,
        max_segment_bytes: DEFAULT_EVENT_SEGMENT_MAX_BYTES,
        max_segments: DEFAULT_EVENT_MAX_SEGMENTS,
        last_write_at: None,
        last_write_error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_scoped_to_state_dir() {
        let options = AcpFileSessionStoreOptions::new("/tmp/example");
        assert_eq!(
            session_event_active_path(&options, "abc"),
            "/tmp/example/sessions/abc.stream.ndjson"
        );
        assert_eq!(
            session_event_segment_path(&options, "abc", 2),
            "/tmp/example/sessions/abc.stream.2.ndjson"
        );
        assert_eq!(
            session_event_lock_path(&options, "abc"),
            "/tmp/example/sessions/abc.stream.lock"
        );
    }

    #[test]
    fn missing_event_log_defaults_are_serde_safe() {
        let value = serde_json::json!({});
        let log: SessionEventLog = serde_json::from_value(value).unwrap_or_default();
        assert_eq!(log.max_segments, DEFAULT_EVENT_MAX_SEGMENTS);
    }
}
