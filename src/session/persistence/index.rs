//! Session index: a small `index.json` cache of per-record summary fields,
//! avoiding a full directory scan + parse of every session file for
//! listing/resolution.
//!
//! Ports `others/acpx/src/session/persistence/index.ts`. Any structural
//! problem with the on-disk index (wrong schema tag, malformed entry, entry
//! count mismatch) is treated as "no usable index" and triggers a full
//! rebuild from the session directory — this is self-healing by
//! construction (the index is a derived cache, never a source of truth), so
//! this port doesn't replicate acpx's per-field partial-validation
//! predicates; a `serde` deserialize failure on any entry already produces
//! the same "rebuild" outcome acpx's manual checks did.

use std::fs;

use serde::{Deserialize, Serialize};

use super::parse::parse_session_record;
use crate::session::record::SessionRecord;
use crate::session::store_options::{AcpFileSessionStoreOptions, ensure_session_dir};

const SESSION_INDEX_SCHEMA: &str = "boltz-acpx.session-index.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIndexEntry {
    pub file: String,
    pub acpx_record_id: String,
    pub acp_session_id: String,
    pub agent_command: String,
    pub cwd: String,
    #[serde(default)]
    pub name: Option<String>,
    pub closed: bool,
    pub last_used_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionIndex {
    schema: String,
    files: Vec<String>,
    entries: Vec<SessionIndexEntry>,
}

/// Ports `toSessionIndexEntry`.
pub fn to_session_index_entry(record: &SessionRecord, file_name: &str) -> SessionIndexEntry {
    SessionIndexEntry {
        file: file_name.to_string(),
        acpx_record_id: record.acpx_record_id.clone(),
        acp_session_id: record.acp_session_id.clone(),
        agent_command: record.agent_command.clone(),
        cwd: record.cwd.clone(),
        name: record.name.clone(),
        closed: record.closed,
        last_used_at: record.last_used_at.clone(),
    }
}

fn index_path(options: &AcpFileSessionStoreOptions) -> std::path::PathBuf {
    options.session_dir().join("index.json")
}

fn list_session_json_files(options: &AcpFileSessionStoreOptions) -> std::io::Result<Vec<String>> {
    let mut files: Vec<String> = fs::read_dir(options.session_dir())?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".json") && name != "index.json")
        .collect();
    files.sort();
    Ok(files)
}

fn read_session_index(options: &AcpFileSessionStoreOptions) -> Option<Vec<SessionIndexEntry>> {
    let payload = fs::read_to_string(index_path(options)).ok()?;
    let index: SessionIndex = serde_json::from_str(&payload).ok()?;
    if index.schema != SESSION_INDEX_SCHEMA {
        return None;
    }
    Some(index.entries)
}

/// Ports `writeSessionIndex`.
pub fn write_session_index(
    options: &AcpFileSessionStoreOptions,
    files: &[String],
    entries: &[SessionIndexEntry],
) -> std::io::Result<()> {
    let mut sorted_files = files.to_vec();
    sorted_files.sort();
    let mut sorted_entries = entries.to_vec();
    sorted_entries.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));

    let index = SessionIndex {
        schema: SESSION_INDEX_SCHEMA.to_string(),
        files: sorted_files,
        entries: sorted_entries,
    };
    let payload = serde_json::to_string_pretty(&index)?;
    let destination = index_path(options);
    let temp = crate::session::store_options::atomic_temp_path(&destination);
    fs::write(&temp, format!("{payload}\n"))?;
    fs::rename(&temp, &destination)
}

/// Ports `rebuildSessionIndex`.
pub fn rebuild_session_index(
    options: &AcpFileSessionStoreOptions,
) -> std::io::Result<Vec<SessionIndexEntry>> {
    let files = list_session_json_files(options)?;
    let mut entries = Vec::new();
    for file in &files {
        let Ok(payload) = fs::read_to_string(options.session_dir().join(file)) else {
            continue;
        };
        let Ok(value) = serde_json::from_str(&payload) else {
            continue;
        };
        if let Some(record) = parse_session_record(&value) {
            entries.push(to_session_index_entry(&record, file));
        }
    }
    write_session_index(options, &files, &entries)?;
    Ok(entries)
}

/// Ports `loadOrRebuildSessionIndex`.
pub fn load_or_rebuild_session_index(
    options: &AcpFileSessionStoreOptions,
) -> std::io::Result<Vec<SessionIndexEntry>> {
    ensure_session_dir(options)?;
    let files = list_session_json_files(options)?;
    if let Some(entries) = read_session_index(options) {
        let cached_files: Vec<String> = entries.iter().map(|e| e.file.clone()).collect();
        // The index also separately tracks a `files` list; approximate the
        // "still fresh" check acpx does by comparing the directory listing
        // against the entry set directly, which is equivalent for a
        // non-corrupt index (every listed file has exactly one entry).
        let mut sorted_cached = cached_files.clone();
        sorted_cached.sort();
        if sorted_cached == files {
            return Ok(entries);
        }
    }
    rebuild_session_index(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn rebuild_reads_files_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        ensure_session_dir(&options).unwrap();

        let record = sample_session_record();
        let value =
            crate::session::persistence::serialize::serialize_session_record_for_disk(&record);
        fs::write(
            options.session_dir().join("record-1.json"),
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .unwrap();

        let entries = rebuild_session_index(&options).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].acpx_record_id, "record-1");
        assert!(options.session_dir().join("index.json").exists());
    }

    #[test]
    fn load_or_rebuild_reuses_fresh_index() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        ensure_session_dir(&options).unwrap();
        let record = sample_session_record();
        let value =
            crate::session::persistence::serialize::serialize_session_record_for_disk(&record);
        fs::write(
            options.session_dir().join("record-1.json"),
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .unwrap();

        let first = load_or_rebuild_session_index(&options).unwrap();
        let second = load_or_rebuild_session_index(&options).unwrap();
        assert_eq!(first, second);
    }
}
