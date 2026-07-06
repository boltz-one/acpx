//! Ports `resolveSessionRecord`: direct-file fast path, then exact-id, then
//! suffix-id index lookup with an explicit ambiguity error.

use std::fs;

use crate::error::{AcpError, Result};
use crate::session::record::SessionRecord;
use crate::session::store_options::{
    AcpFileSessionStoreOptions, ensure_session_dir, session_file_path,
};

use super::load_record_from_file;
use crate::session::persistence::index::load_or_rebuild_session_index;
use crate::session::persistence::parse::parse_session_record;

pub fn resolve_session_record(
    options: &AcpFileSessionStoreOptions,
    session_id: &str,
) -> Result<SessionRecord> {
    ensure_session_dir(options).map_err(|err| AcpError::Other(err.into()))?;

    let direct_path = session_file_path(options, session_id);
    if let Ok(payload) = fs::read_to_string(&direct_path) {
        if let Ok(value) = serde_json::from_str(&payload) {
            if let Some(record) = parse_session_record(&value) {
                return Ok(record);
            }
        }
    }

    let entries =
        load_or_rebuild_session_index(options).map_err(|err| AcpError::Other(err.into()))?;

    let exact: Vec<SessionRecord> = entries
        .iter()
        .filter(|entry| entry.acpx_record_id == session_id || entry.acp_session_id == session_id)
        .filter_map(|entry| load_record_from_file(options, &entry.file))
        .collect();
    if exact.len() == 1 {
        return Ok(exact.into_iter().next().unwrap());
    }
    if exact.len() > 1 {
        return Err(AcpError::SessionResolution(format!(
            "Multiple sessions match id: {session_id}"
        )));
    }

    let suffix: Vec<SessionRecord> = entries
        .iter()
        .filter(|entry| {
            entry.acpx_record_id.ends_with(session_id) || entry.acp_session_id.ends_with(session_id)
        })
        .filter_map(|entry| load_record_from_file(options, &entry.file))
        .collect();
    if suffix.len() == 1 {
        return Ok(suffix.into_iter().next().unwrap());
    }
    if suffix.len() > 1 {
        return Err(AcpError::SessionResolution(format!(
            "Session id is ambiguous: {session_id}"
        )));
    }

    Err(AcpError::SessionNotFound {
        session_id: session_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::repository::write_session_record;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn resolve_missing_session_reports_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        let err = resolve_session_record(&options, "does-not-exist").unwrap_err();
        assert!(matches!(err, AcpError::SessionNotFound { .. }));
    }

    #[test]
    fn suffix_resolution_is_ambiguous_with_two_matches() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());

        let mut a = sample_session_record();
        a.acpx_record_id = "session-aaaa-shared".to_string();
        let mut b = sample_session_record();
        b.acpx_record_id = "session-bbbb-shared".to_string();
        write_session_record(&options, &a).unwrap();
        write_session_record(&options, &b).unwrap();

        let err = resolve_session_record(&options, "shared").unwrap_err();
        assert!(matches!(err, AcpError::SessionResolution(_)));
    }

    #[test]
    fn suffix_resolution_succeeds_with_one_match() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        let mut record = sample_session_record();
        record.acpx_record_id = "session-unique-suffix".to_string();
        write_session_record(&options, &record).unwrap();

        let resolved = resolve_session_record(&options, "unique-suffix").unwrap();
        assert_eq!(resolved.acpx_record_id, record.acpx_record_id);
    }
}
