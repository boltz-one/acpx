//! Building the new local [`SessionRecord`] for an import, and guarding
//! against destination/provider-id collisions.
//!
//! Ports `generateRecordId`, `resolveImportedCwd`, `buildImportedRecord`,
//! `assertDestinationScopeAvailable`, `assertProviderSessionAvailable` from
//! `others/acpx/src/session/import.ts`.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::session::event_log::default_session_event_log;
use crate::session::export::ExportedSession;
use crate::session::persistence::repository::{
    FindSessionOptions, absolute_path, find_session, list_sessions,
};
use crate::session::record::{SessionImportedFrom, SessionRecord};
use crate::session::store_options::AcpFileSessionStoreOptions;

use super::import_error;

pub(super) fn generate_record_id(sessions_dir: &Path) -> String {
    loop {
        let candidate = uuid::Uuid::new_v4().to_string();
        let path = sessions_dir.join(format!(
            "{}.json",
            crate::session::store_options::safe_session_id(&candidate)
        ));
        if !path.exists() {
            return candidate;
        }
    }
}

pub(super) fn resolve_imported_cwd(cwd_relative: &str, new_cwd: Option<&Path>) -> PathBuf {
    if let Some(new_cwd) = new_cwd {
        return absolute_path(new_cwd);
    }
    let candidate = Path::new(cwd_relative);
    if candidate.is_absolute() {
        return candidate.to_path_buf();
    }
    dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(candidate)
}

pub(super) fn build_imported_record(
    archive: &ExportedSession,
    source_record: SessionRecord,
    options: &AcpFileSessionStoreOptions,
    new_record_id: &str,
    cwd: &str,
    name: Option<String>,
) -> SessionRecord {
    let has_history = !archive.history.is_empty();
    let mut event_log = default_session_event_log(options, new_record_id);
    event_log.max_segment_bytes = source_record.event_log.max_segment_bytes;
    event_log.max_segments = source_record.event_log.max_segments;
    event_log.segment_count = if has_history {
        1
    } else {
        source_record.event_log.segment_count
    };

    SessionRecord {
        acpx_record_id: new_record_id.to_string(),
        cwd: cwd.to_string(),
        name: name.or_else(|| archive.session.name.clone()),
        closed: false,
        closed_at: None,
        pid: None,
        agent_started_at: None,
        last_agent_exit_code: None,
        last_agent_exit_signal: None,
        last_agent_exit_at: None,
        last_agent_disconnect_reason: None,
        event_log,
        imported_from: Some(SessionImportedFrom {
            record_id: archive.session.record_id.clone(),
            cwd_original: archive.session.cwd_original.clone(),
            exported_by: archive.exported_by.clone(),
            exported_at: archive.exported_at.clone(),
        }),
        ..source_record
    }
}

pub(super) fn assert_destination_scope_available(
    options: &AcpFileSessionStoreOptions,
    record: &SessionRecord,
) -> Result<()> {
    let existing = find_session(
        options,
        &FindSessionOptions {
            agent_command: record.agent_command.clone(),
            cwd: PathBuf::from(&record.cwd),
            name: record.name.clone(),
            include_closed: false,
        },
    )?;
    if existing.is_some() {
        return Err(import_error(
            "A session already exists for the import destination scope; pass a different name or cwd",
        ));
    }
    Ok(())
}

pub(super) fn assert_provider_session_available(
    options: &AcpFileSessionStoreOptions,
    record: &SessionRecord,
) -> Result<()> {
    let existing = list_sessions(options)?
        .into_iter()
        .any(|session| session.acp_session_id == record.acp_session_id);
    if existing {
        return Err(import_error(
            "A local session already uses this provider session id; prune or remove the existing record first",
        ));
    }
    Ok(())
}
