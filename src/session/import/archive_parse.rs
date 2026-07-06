//! Parsing + format-version validation of an export archive file.
//!
//! Ports the `parseArchive`/`assertSupportedFormatVersion` half of
//! `others/acpx/src/session/import.ts`.

use serde_json::Value;

use crate::error::Result;
use crate::session::export::ExportedSession;

use super::import_error;

const SUPPORTED_FORMAT_VERSION: u32 = 1;

pub(super) fn parse_archive(raw: &str) -> Result<ExportedSession> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|err| import_error(format!("Invalid session export archive JSON: {err}")))?;
    let format_version = value.get("format_version").and_then(Value::as_u64);
    if format_version != Some(SUPPORTED_FORMAT_VERSION as u64) {
        return Err(import_error(format!(
            "Unsupported session export format_version {format_version:?}; supported version is {SUPPORTED_FORMAT_VERSION}"
        )));
    }
    serde_json::from_value(value)
        .map_err(|err| import_error(format!("Invalid session export archive: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_archive_with_unsupported_format_version() {
        let raw = serde_json::json!({"format_version": 99}).to_string();
        assert!(parse_archive(&raw).is_err());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse_archive("{not json").is_err());
    }
}
