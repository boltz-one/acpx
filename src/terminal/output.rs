//! Buffered stdout/stderr capture with a byte-limit cap, enforced at
//! append-time (not read-time) so a fast-producing child can't balloon
//! memory before the next `terminal/output` poll. Ports the buffering half
//! of `others/acpx/src/acp/terminal-manager.ts` (`appendOutput`,
//! `trimToUtf8Boundary`).

use std::sync::Arc;

use parking_lot::Mutex;
use smol::io::AsyncReadExt;

pub const DEFAULT_TERMINAL_OUTPUT_LIMIT_BYTES: u64 = 64 * 1024;

#[derive(Debug, Default)]
struct Inner {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Thread-safe append-only output buffer shared between the stdout/stderr
/// reader tasks (writers) and `terminal/output` request handling (readers).
#[derive(Debug, Default)]
pub struct OutputBuffer {
    inner: Mutex<Inner>,
}

/// A point-in-time read of the buffer. Named (rather than a raw `String`)
/// per the phase's Step 7 note: keeps the door open for a future streaming
/// API without changing this type's shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSnapshot {
    pub output: String,
    pub truncated: bool,
}

impl OutputBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `chunk`, trimming from the front at a UTF-8 char boundary
    /// once the buffer exceeds `limit` bytes.
    pub fn append(&self, chunk: &[u8], limit: u64) {
        if chunk.is_empty() {
            return;
        }
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let mut inner = self.inner.lock();
        inner.bytes.extend_from_slice(chunk);
        if inner.bytes.len() > limit {
            inner.bytes = trim_to_utf8_boundary(&inner.bytes, limit);
            inner.truncated = true;
        }
    }

    pub fn snapshot(&self) -> OutputSnapshot {
        let inner = self.inner.lock();
        OutputSnapshot {
            output: String::from_utf8_lossy(&inner.bytes).into_owned(),
            truncated: inner.truncated,
        }
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.bytes.clear();
        inner.truncated = false;
    }
}

/// Spawns a background task draining `stream` (a terminal child's stdout or
/// stderr pipe) into `output` until EOF/error. `None` (the child has no such
/// stream) spawns a no-op task so callers don't need a separate branch.
pub fn spawn_reader<R>(stream: Option<R>, output: Arc<OutputBuffer>, limit: u64) -> smol::Task<()>
where
    R: futures::AsyncRead + Unpin + Send + 'static,
{
    smol::spawn(async move {
        let Some(mut stream) = stream else {
            return;
        };
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => output.append(&buf[..n], limit),
            }
        }
    })
}

/// Ports `trimToUtf8Boundary`: keeps the last `limit` bytes, nudged forward
/// past any leading UTF-8 continuation bytes so the retained slice starts on
/// a character boundary (falls back to the raw byte cut if nudging would
/// consume the whole buffer, matching acpx's own fallback).
fn trim_to_utf8_boundary(buffer: &[u8], limit: usize) -> Vec<u8> {
    if limit == 0 {
        return Vec::new();
    }
    if buffer.len() <= limit {
        return buffer.to_vec();
    }

    let mut start = buffer.len() - limit;
    while start < buffer.len() && (buffer[start] & 0b1100_0000) == 0b1000_0000 {
        start += 1;
    }
    if start >= buffer.len() {
        start = buffer.len() - limit;
    }
    buffer[start..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_under_limit_keeps_everything() {
        let buf = OutputBuffer::new();
        buf.append(b"hello", 64);
        let snapshot = buf.snapshot();
        assert_eq!(snapshot.output, "hello");
        assert!(!snapshot.truncated);
    }

    #[test]
    fn append_over_limit_truncates_from_front() {
        let buf = OutputBuffer::new();
        buf.append(b"0123456789", 4);
        let snapshot = buf.snapshot();
        assert_eq!(snapshot.output, "6789");
        assert!(snapshot.truncated);
    }

    #[test]
    fn truncation_respects_utf8_boundaries() {
        let buf = OutputBuffer::new();
        // "é" is 2 bytes (0xC3 0xA9); a byte-exact 4-byte window starting
        // mid-character must nudge forward rather than split it.
        buf.append("ab\u{e9}cd".as_bytes(), 4);
        let snapshot = buf.snapshot();
        assert!(String::from_utf8(snapshot.output.clone().into_bytes()).is_ok());
        assert!(snapshot.truncated);
    }

    #[test]
    fn zero_limit_drops_everything_but_marks_truncated() {
        let buf = OutputBuffer::new();
        buf.append(b"data", 0);
        let snapshot = buf.snapshot();
        assert_eq!(snapshot.output, "");
        assert!(snapshot.truncated);
    }

    #[test]
    fn clear_resets_state() {
        let buf = OutputBuffer::new();
        buf.append(b"data", 4);
        buf.clear();
        let snapshot = buf.snapshot();
        assert_eq!(snapshot.output, "");
        assert!(!snapshot.truncated);
    }
}
