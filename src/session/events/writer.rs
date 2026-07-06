//! Ports `SessionEventWriter` from `others/acpx/src/session/events.ts`.

use std::fs::OpenOptions;
use std::io::Write;

use serde_json::Value;

use crate::error::{AcpError, Result};
use crate::session::event_log::{
    DEFAULT_EVENT_MAX_SEGMENTS, DEFAULT_EVENT_SEGMENT_MAX_BYTES, SessionEventLog,
    session_event_active_path,
};
use crate::session::record::SessionRecord;
use crate::session::store_options::AcpFileSessionStoreOptions;

use super::lock::{LockHandle, acquire_events_lock, release_events_lock};
use super::now_iso;
use super::rotate::{resolve_initial_segment_count, rotate_segments, stat_size};

#[derive(Debug, Clone, Copy, Default)]
pub struct SessionEventWriterOptions {
    pub max_segment_bytes: Option<u64>,
    pub max_segments: Option<u32>,
}

pub struct SessionEventWriter<'a> {
    options: &'a AcpFileSessionStoreOptions,
    record: SessionRecord,
    lock: LockHandle,
    max_segment_bytes: u64,
    max_segments: u32,
    active_path: String,
    active_size_bytes: u64,
    segment_count: u32,
    closed: bool,
}

impl<'a> SessionEventWriter<'a> {
    /// Ports `SessionEventWriter.open`.
    pub fn open(
        options: &'a AcpFileSessionStoreOptions,
        record: SessionRecord,
        writer_options: SessionEventWriterOptions,
    ) -> Result<Self> {
        let lock = acquire_events_lock(options, &record.acpx_record_id)?;
        let max_segment_bytes =
            writer_options
                .max_segment_bytes
                .unwrap_or(if record.event_log.max_segment_bytes > 0 {
                    record.event_log.max_segment_bytes
                } else {
                    DEFAULT_EVENT_SEGMENT_MAX_BYTES
                });
        let max_segments =
            writer_options
                .max_segments
                .unwrap_or(if record.event_log.max_segments > 0 {
                    record.event_log.max_segments
                } else {
                    DEFAULT_EVENT_MAX_SEGMENTS
                });
        let active_path = session_event_active_path(options, &record.acpx_record_id);
        let active_size_bytes = stat_size(&active_path);
        let segment_count = resolve_initial_segment_count(options, &record, max_segments);

        Ok(Self {
            options,
            record,
            lock,
            max_segment_bytes,
            max_segments,
            active_path,
            active_size_bytes,
            segment_count,
            closed: false,
        })
    }

    pub fn record(&self) -> &SessionRecord {
        &self.record
    }

    /// Ports `appendMessages`.
    pub fn append_messages(&mut self, messages: &[Value], checkpoint: bool) -> Result<()> {
        if self.closed {
            return Err(AcpError::Other(anyhow::anyhow!(
                "SessionEventWriter is closed"
            )));
        }
        if messages.is_empty() {
            return Ok(());
        }
        crate::session::store_options::ensure_session_dir(self.options)
            .map_err(|err| AcpError::Other(err.into()))?;

        for message in messages {
            self.append_one(message)?;
        }

        if checkpoint {
            crate::session::persistence::repository::write_session_record(
                self.options,
                &self.record,
            )?;
        }
        Ok(())
    }

    fn append_one(&mut self, message: &Value) -> Result<()> {
        let line = format!("{message}\n");
        let line_bytes = line.len() as u64;
        if self.active_size_bytes > 0
            && self.active_size_bytes + line_bytes > self.max_segment_bytes
        {
            rotate_segments(self.options, &self.record.acpx_record_id, self.max_segments)?;
            self.active_path = session_event_active_path(self.options, &self.record.acpx_record_id);
            self.active_size_bytes = 0;
            self.segment_count = (self.segment_count + 1).min(self.max_segments);
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.active_path)
            .map_err(|err| AcpError::Other(err.into()))?;
        file.write_all(line.as_bytes())
            .map_err(|err| AcpError::Other(err.into()))?;
        self.active_size_bytes += line_bytes;

        self.record.last_seq += 1;
        if let Some(id) = message.get("id") {
            if id.is_string() || id.is_number() {
                self.record.last_request_id = Some(id.to_string());
            }
        }
        let write_ts = now_iso();
        self.record.last_used_at = write_ts.clone();
        self.record.event_log = SessionEventLog {
            active_path: self.active_path.clone(),
            segment_count: self.segment_count,
            max_segment_bytes: self.max_segment_bytes,
            max_segments: self.max_segments,
            last_write_at: Some(write_ts),
            last_write_error: None,
        };
        Ok(())
    }

    pub fn append_message(&mut self, message: &Value, checkpoint: bool) -> Result<()> {
        self.append_messages(std::slice::from_ref(message), checkpoint)
    }

    /// Ports `checkpoint()`.
    pub fn checkpoint(&self) -> Result<()> {
        if self.closed {
            return Err(AcpError::Other(anyhow::anyhow!(
                "SessionEventWriter is closed"
            )));
        }
        crate::session::persistence::repository::write_session_record(self.options, &self.record)
    }

    /// Ports `close()`.
    pub fn close(&mut self, checkpoint: bool) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let result = if checkpoint {
            crate::session::persistence::repository::write_session_record(
                self.options,
                &self.record,
            )
        } else {
            Ok(())
        };
        self.closed = true;
        release_events_lock(&self.lock);
        result
    }
}

impl Drop for SessionEventWriter<'_> {
    fn drop(&mut self) {
        if !self.closed {
            release_events_lock(&self.lock);
        }
    }
}
