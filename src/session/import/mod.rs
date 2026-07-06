//! Importing a session archive produced by [`super::export::export_session`].
//!
//! Ports `others/acpx/src/session/import.ts`, split across [`archive_parse`]
//! (format-version validation), [`agent_match`] (agent-identity checks),
//! and [`build`] (new-record construction + collision guards) to stay
//! under this crate's per-file line convention.

mod agent_match;
mod archive_parse;
mod build;

use std::path::{Path, PathBuf};

use crate::error::{AcpError, Result};
use crate::session::event_log::session_event_active_path;
use crate::session::persistence::parse::parse_session_record;
use crate::session::persistence::repository::write_session_record;
use crate::session::store_options::AcpFileSessionStoreOptions;

#[derive(Debug, Clone, Default)]
pub struct ImportSessionOptions {
    pub name: Option<String>,
    pub new_cwd: Option<PathBuf>,
    pub expected_agent_name: Option<String>,
    pub expected_agent_command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSession {
    pub record_id: String,
    pub cwd: String,
}

fn import_error(message: impl Into<String>) -> AcpError {
    AcpError::SessionResolution(message.into())
}

/// Ports `importSession`.
pub fn import_session(
    options: &AcpFileSessionStoreOptions,
    archive_path: &Path,
    import_options: &ImportSessionOptions,
) -> Result<ImportedSession> {
    let raw = std::fs::read_to_string(archive_path).map_err(|err| AcpError::Other(err.into()))?;
    let archive = archive_parse::parse_archive(&raw)?;

    let mut source_record = parse_session_record(&archive.session.state).ok_or_else(|| {
        import_error("Invalid session export archive: session.state is not a session record")
    })?;
    agent_match::assert_expected_agent_command(&archive, &mut source_record, import_options)?;

    crate::session::store_options::ensure_session_dir(options)
        .map_err(|err| AcpError::Other(err.into()))?;
    let sessions_dir = options.session_dir();

    let cwd = build::resolve_imported_cwd(
        &archive.session.cwd_relative,
        import_options.new_cwd.as_deref(),
    );
    let new_record_id = build::generate_record_id(&sessions_dir);
    let new_record = build::build_imported_record(
        &archive,
        source_record,
        options,
        &new_record_id,
        &cwd.to_string_lossy(),
        import_options.name.clone(),
    );

    build::assert_destination_scope_available(options, &new_record)?;
    build::assert_provider_session_available(options, &new_record)?;
    write_session_record(options, &new_record)?;

    if !archive.history.is_empty() {
        let lines: Vec<String> = archive
            .history
            .iter()
            .map(|entry| entry.to_string())
            .collect();
        std::fs::write(
            session_event_active_path(options, &new_record_id),
            format!("{}\n", lines.join("\n")),
        )
        .map_err(|err| AcpError::Other(err.into()))?;
    }

    Ok(ImportedSession {
        record_id: new_record_id,
        cwd: cwd.to_string_lossy().into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::export::{SessionExportLookup, export_session};
    use crate::session::persistence::repository::{
        resolve_session_record, write_session_record as write_record,
    };
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn round_trips_export_then_import() {
        let export_dir = tempfile::tempdir().unwrap();
        let export_options = AcpFileSessionStoreOptions::new(export_dir.path());
        let mut record = sample_session_record();
        record.closed = true;
        record.cwd = "/tmp/round-trip-project".to_string();
        // The export lookup below doesn't specify a `name`.
        record.name = None;
        write_record(&export_options, &record).unwrap();

        let archive_path = export_dir.path().join("archive.json");
        export_session(
            &export_options,
            &SessionExportLookup {
                cwd: Some(PathBuf::from("/tmp/round-trip-project")),
                ..Default::default()
            },
            &archive_path,
        )
        .unwrap();

        let import_dir = tempfile::tempdir().unwrap();
        let import_options = AcpFileSessionStoreOptions::new(import_dir.path());
        let imported = import_session(
            &import_options,
            &archive_path,
            &ImportSessionOptions::default(),
        )
        .unwrap();

        let resolved = resolve_session_record(&import_options, &imported.record_id).unwrap();
        assert_eq!(resolved.agent_command, record.agent_command);
        assert!(resolved.imported_from.is_some());
        assert_eq!(
            resolved.imported_from.unwrap().record_id,
            record.acpx_record_id
        );
    }
}
