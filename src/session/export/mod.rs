//! Exporting a session record + its event-log history to a portable
//! archive file.
//!
//! Ports `others/acpx/src/session/export.ts`. JSON-RPC history entries are
//! kept as opaque `serde_json::Value`s rather than acpx's typed
//! `AcpJsonRpcMessage` (`isAcpJsonRpcMessage` lives in `acp/jsonrpc.ts`,
//! Phase 2 territory this phase doesn't depend on) — this preserves the
//! forward-compat spirit of ADR-5 (an opaque, round-trippable blob) without
//! introducing a dependency this phase shouldn't have. Split across
//! [`lookup`] (session lookup) and [`archive`] (history/liveness/redaction)
//! to stay under this crate's per-file line convention.

mod archive;
mod lookup;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AcpError, Result};
use crate::session::store_options::AcpFileSessionStoreOptions;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedSessionInfo {
    pub record_id: String,
    pub name: Option<String>,
    pub agent: String,
    #[serde(default)]
    pub agent_name: Option<String>,
    pub cwd_relative: String,
    pub cwd_original: String,
    pub created_at: String,
    pub updated_at: String,
    pub state: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedSession {
    pub format_version: u32,
    pub exported_at: String,
    pub exported_by: String,
    pub session: ExportedSessionInfo,
    pub history: Vec<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionExportLookup {
    pub agent_name: Option<String>,
    pub agent_command: Option<String>,
    pub cwd: Option<PathBuf>,
    pub name: Option<String>,
}

fn lookup_error(message: impl Into<String>) -> AcpError {
    AcpError::SessionResolution(message.into())
}

/// Ports `normalizeAgentName` (the export/import-local one, distinct from
/// `agent_command::normalize_agent_name`): lowercases and folds the
/// `factory-droid`/`factorydroid` aliases to `droid`.
pub fn normalize_agent_identity(agent_name: Option<&str>) -> Option<String> {
    let normalized = agent_name?.trim().to_lowercase();
    if normalized.is_empty() {
        return None;
    }
    Some(match normalized.as_str() {
        "factory-droid" | "factorydroid" => "droid".to_string(),
        _ => normalized,
    })
}

/// Ports `exportSession`.
pub fn export_session(
    options: &AcpFileSessionStoreOptions,
    lookup: &SessionExportLookup,
    output_path: &Path,
) -> Result<()> {
    let record = lookup::load_session_record(options, lookup)?
        .ok_or_else(|| lookup_error("session not found"))?;

    if archive::is_session_active(options, &record) {
        return Err(lookup_error(
            "session is currently locked by a running queue owner; close it first",
        ));
    }

    let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let cwd_relative = archive::cwd_relative_to_home(&record.cwd, &home);
    let exported = ExportedSession {
        format_version: 1,
        exported_at: crate::session::conversation_model::iso_now(),
        exported_by: "boltz-acpx".to_string(),
        session: ExportedSessionInfo {
            record_id: record.acpx_record_id.clone(),
            name: record.name.clone(),
            agent: record.agent_command.clone(),
            agent_name: normalize_agent_identity(lookup.agent_name.as_deref()),
            cwd_relative: cwd_relative.clone(),
            cwd_original: cwd_relative.clone(),
            created_at: record.created_at.clone(),
            updated_at: record.last_used_at.clone(),
            state: archive::serialize_session_record_for_archive(&record, &cwd_relative),
        },
        history: archive::read_session_history(options, &record),
    };

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| AcpError::Other(err.into()))?;
    }
    let payload =
        serde_json::to_string_pretty(&exported).map_err(|err| AcpError::Other(err.into()))?;
    std::fs::write(output_path, format!("{payload}\n"))
        .map_err(|err| AcpError::Other(err.into()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::repository::write_session_record;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn exports_a_closed_session_to_an_archive_file() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        let mut record = sample_session_record();
        record.closed = true;
        record.cwd = "/tmp/exported-project".to_string();
        // The lookup below doesn't specify a `name`, so it only matches an
        // unnamed session (mirrors acpx's exact name-scope matching).
        record.name = None;
        write_session_record(&options, &record).unwrap();

        let output = dir.path().join("archive.json");
        export_session(
            &options,
            &SessionExportLookup {
                cwd: Some(PathBuf::from("/tmp/exported-project")),
                ..Default::default()
            },
            &output,
        )
        .unwrap();

        let payload = std::fs::read_to_string(&output).unwrap();
        let exported: ExportedSession = serde_json::from_str(&payload).unwrap();
        assert_eq!(exported.format_version, 1);
        assert_eq!(exported.session.record_id, record.acpx_record_id);
    }

    #[test]
    #[cfg(unix)]
    fn refuses_to_export_a_live_session() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("failed to spawn sleep");
        let mut record = sample_session_record();
        record.closed = false;
        record.pid = Some(child.id());
        record.cwd = "/tmp/live-project".to_string();
        record.name = None;
        write_session_record(&options, &record).unwrap();

        let err = export_session(
            &options,
            &SessionExportLookup {
                cwd: Some(PathBuf::from("/tmp/live-project")),
                ..Default::default()
            },
            &dir.path().join("archive.json"),
        )
        .unwrap_err();
        assert!(matches!(err, AcpError::SessionResolution(_)));

        let _ = child.kill();
        let _ = child.wait();
    }
}
