//! Segment counting/rotation for the append-only event log.
//!
//! Ports `countExistingSegments`, `resolveInitialSegmentCount`,
//! `rotateSegments` from `others/acpx/src/session/events.ts`.

use std::fs;

use crate::error::{AcpError, Result};
use crate::session::event_log::{session_event_active_path, session_event_segment_path};
use crate::session::record::SessionRecord;
use crate::session::store_options::AcpFileSessionStoreOptions;

pub(super) fn path_exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

pub(super) fn stat_size(path: &str) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn count_existing_segments(
    options: &AcpFileSessionStoreOptions,
    session_id: &str,
    max_segments: u32,
) -> u32 {
    let mut count = 0;
    for segment in 1..=max_segments {
        if path_exists(&session_event_segment_path(options, session_id, segment)) {
            count += 1;
        }
    }
    if path_exists(&session_event_active_path(options, session_id)) {
        count += 1;
    }
    count
}

pub(super) fn resolve_initial_segment_count(
    options: &AcpFileSessionStoreOptions,
    record: &SessionRecord,
    max_segments: u32,
) -> u32 {
    if record.event_log.segment_count > 0 {
        return record.event_log.segment_count;
    }
    let existing = count_existing_segments(options, &record.acpx_record_id, max_segments);
    if existing > 0 { existing } else { 1 }
}

/// Ports `rotateSegments`.
pub(super) fn rotate_segments(
    options: &AcpFileSessionStoreOptions,
    session_id: &str,
    max_segments: u32,
) -> Result<()> {
    let active = session_event_active_path(options, session_id);

    let overflow = session_event_segment_path(options, session_id, max_segments);
    if path_exists(&overflow) {
        fs::remove_file(&overflow).map_err(|err| AcpError::Other(err.into()))?;
    }

    for segment in (1..max_segments).rev() {
        let from = session_event_segment_path(options, session_id, segment);
        let to = session_event_segment_path(options, session_id, segment + 1);
        if path_exists(&from) {
            fs::rename(&from, &to).map_err(|err| AcpError::Other(err.into()))?;
        }
    }

    if path_exists(&active) {
        fs::rename(&active, session_event_segment_path(options, session_id, 1))
            .map_err(|err| AcpError::Other(err.into()))?;
    }
    Ok(())
}
