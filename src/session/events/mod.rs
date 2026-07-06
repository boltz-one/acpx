//! Append-only NDJSON event log for a session: segment rotation, a
//! same-process/cross-process advisory lock, and listing.
//!
//! Ports `others/acpx/src/session/events.ts`. As with [`super::export`]/
//! [`super::import`], appended messages are opaque `serde_json::Value`s
//! rather than acpx's typed `AcpJsonRpcMessage` (Phase 2 territory this
//! phase doesn't depend on). File I/O is synchronous `std::fs` (this
//! module's contract is "small, infrequent, local-disk metadata/log
//! writes", the same class of operation [`super::persistence::repository`]
//! already performs synchronously) rather than threaded through `smol`'s
//! async I/O, keeping this phase's file-ownership self-contained. Split
//! across [`lock`], [`rotate`], [`writer`] to stay under this crate's
//! per-file line convention.

mod lock;
mod rotate;
mod writer;

use serde_json::Value;

use crate::session::event_log::{
    DEFAULT_EVENT_MAX_SEGMENTS, session_event_active_path, session_event_segment_path,
};
use crate::session::record::SessionRecord;
use crate::session::store_options::AcpFileSessionStoreOptions;

pub use writer::{SessionEventWriter, SessionEventWriterOptions};

fn now_iso() -> String {
    crate::session::conversation_model::iso_now()
}

/// Ports `listSessionEvents`.
pub fn list_session_events(
    options: &AcpFileSessionStoreOptions,
    record: &SessionRecord,
) -> Vec<Value> {
    let max_segments = if record.event_log.max_segments > 0 {
        record.event_log.max_segments
    } else {
        DEFAULT_EVENT_MAX_SEGMENTS
    };

    let mut files = Vec::new();
    for segment in (1..=max_segments).rev() {
        let path = session_event_segment_path(options, &record.acpx_record_id, segment);
        if rotate::path_exists(&path) {
            files.push(path);
        }
    }
    let active = session_event_active_path(options, &record.acpx_record_id);
    if rotate::path_exists(&active) {
        files.push(active);
    }

    let mut events = Vec::new();
    for file in files {
        let Ok(payload) = std::fs::read_to_string(&file) else {
            continue;
        };
        for line in payload.lines().filter(|l| !l.trim().is_empty()) {
            if let Ok(value) = serde_json::from_str(line) {
                events.push(value);
            }
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn appends_and_lists_events() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        let record = sample_session_record();

        {
            let mut writer = SessionEventWriter::open(
                &options,
                record.clone(),
                SessionEventWriterOptions::default(),
            )
            .unwrap();
            writer
                .append_message(
                    &serde_json::json!({"jsonrpc": "2.0", "method": "ping"}),
                    false,
                )
                .unwrap();
            writer.close(true).unwrap();
        }

        let events = list_session_events(&options, &record);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["method"], "ping");
    }

    #[test]
    fn rotates_segments_once_max_bytes_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        let mut record = sample_session_record();
        record.acpx_record_id = "rotate-test".to_string();

        let mut writer = SessionEventWriter::open(
            &options,
            record,
            SessionEventWriterOptions {
                max_segment_bytes: Some(10),
                max_segments: Some(2),
            },
        )
        .unwrap();
        writer
            .append_message(&serde_json::json!({"a": 1}), false)
            .unwrap();
        writer
            .append_message(&serde_json::json!({"b": 2}), false)
            .unwrap();
        writer.close(false).unwrap();

        assert!(
            options
                .session_dir()
                .join("rotate-test.stream.1.ndjson")
                .exists()
        );
    }

    #[test]
    fn lock_is_released_after_close_allowing_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        let record = sample_session_record();

        let mut writer = SessionEventWriter::open(
            &options,
            record.clone(),
            SessionEventWriterOptions::default(),
        )
        .unwrap();
        writer.close(false).unwrap();

        let mut writer2 =
            SessionEventWriter::open(&options, record, SessionEventWriterOptions::default())
                .unwrap();
        writer2.close(false).unwrap();
    }
}
