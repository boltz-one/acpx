//! Ports `listSessions`/`listSessionsForAgent`/`findSession`/
//! `findSessionByDirectoryWalk`/`findGitRepositoryRoot`.

use std::path::{Path, PathBuf};

use crate::error::{AcpError, Result};
use crate::session::record::SessionRecord;
use crate::session::store_options::{AcpFileSessionStoreOptions, ensure_session_dir};

use super::{
    FindSessionByDirectoryWalkOptions, FindSessionOptions, absolute_path, is_within_boundary,
    load_record_from_file, matches_session_entry, normalize_name,
};
use crate::session::persistence::index::{SessionIndexEntry, load_or_rebuild_session_index};

pub fn list_sessions(options: &AcpFileSessionStoreOptions) -> Result<Vec<SessionRecord>> {
    ensure_session_dir(options).map_err(|err| AcpError::Other(err.into()))?;
    let entries =
        load_or_rebuild_session_index(options).map_err(|err| AcpError::Other(err.into()))?;
    let mut records: Vec<SessionRecord> = entries
        .iter()
        .filter_map(|entry| load_record_from_file(options, &entry.file))
        .collect();
    records.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
    Ok(records)
}

pub fn list_sessions_for_agent(
    options: &AcpFileSessionStoreOptions,
    agent_command: &str,
) -> Result<Vec<SessionRecord>> {
    let entries =
        load_or_rebuild_session_index(options).map_err(|err| AcpError::Other(err.into()))?;
    let mut records: Vec<SessionRecord> = entries
        .iter()
        .filter(|entry| entry.agent_command == agent_command)
        .filter_map(|entry| load_record_from_file(options, &entry.file))
        .collect();
    records.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
    Ok(records)
}

pub fn find_session(
    options: &AcpFileSessionStoreOptions,
    query: &FindSessionOptions,
) -> Result<Option<SessionRecord>> {
    let normalized_cwd = absolute_path(&query.cwd);
    let normalized_cwd = normalized_cwd.to_string_lossy();
    let normalized_name = normalize_name(query.name.as_deref());
    let entries =
        load_or_rebuild_session_index(options).map_err(|err| AcpError::Other(err.into()))?;
    let found = entries.iter().find(|entry| {
        entry.agent_command == query.agent_command
            && matches_session_entry(
                entry,
                &normalized_cwd,
                normalized_name.as_deref(),
                query.include_closed,
            )
    });
    Ok(found.and_then(|entry| load_record_from_file(options, &entry.file)))
}

pub fn find_session_by_directory_walk(
    options: &AcpFileSessionStoreOptions,
    query: &FindSessionByDirectoryWalkOptions,
) -> Result<Option<SessionRecord>> {
    let normalized_name = normalize_name(query.name.as_deref());
    let normalized_start = absolute_path(&query.cwd);
    let normalized_boundary = query
        .boundary
        .as_deref()
        .map(absolute_path)
        .unwrap_or_else(|| normalized_start.clone());
    let walk_boundary = if is_within_boundary(&normalized_boundary, &normalized_start) {
        normalized_boundary
    } else {
        normalized_start.clone()
    };

    let entries =
        load_or_rebuild_session_index(options).map_err(|err| AcpError::Other(err.into()))?;
    let candidates: Vec<&SessionIndexEntry> = entries
        .iter()
        .filter(|entry| entry.agent_command == query.agent_command)
        .collect();

    let mut current = normalized_start;
    loop {
        let current_str = current.to_string_lossy();
        if let Some(entry) = candidates.iter().find(|entry| {
            matches_session_entry(entry, &current_str, normalized_name.as_deref(), false)
        }) {
            return Ok(load_record_from_file(options, &entry.file));
        }

        if current == walk_boundary {
            return Ok(None);
        }
        let Some(parent) = current.parent().map(Path::to_path_buf) else {
            return Ok(None);
        };
        if parent == current || !is_within_boundary(&walk_boundary, &parent) {
            return Ok(None);
        }
        current = parent;
    }
}

/// Ports `findGitRepositoryRoot`.
pub fn find_git_repository_root(start_dir: &Path) -> Option<PathBuf> {
    let mut current = absolute_path(start_dir);
    loop {
        if current.join(".git").is_dir() {
            return Some(current);
        }
        let parent = current.parent()?.to_path_buf();
        if parent == current {
            return None;
        }
        current = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::repository::write_session_record;
    use crate::session::persistence::serialize::test_support::sample_session_record;

    #[test]
    fn list_sessions_orders_by_last_used_descending() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        let mut older = sample_session_record();
        older.acpx_record_id = "older".to_string();
        older.last_used_at = "2025-01-01T00:00:00Z".to_string();
        let mut newer = sample_session_record();
        newer.acpx_record_id = "newer".to_string();
        newer.last_used_at = "2026-01-01T00:00:00Z".to_string();
        write_session_record(&options, &older).unwrap();
        write_session_record(&options, &newer).unwrap();

        let records = list_sessions(&options).unwrap();
        assert_eq!(records[0].acpx_record_id, "newer");
        assert_eq!(records[1].acpx_record_id, "older");
    }

    #[test]
    fn find_session_matches_by_agent_and_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let options = AcpFileSessionStoreOptions::new(dir.path());
        let mut record = sample_session_record();
        record.cwd = "/tmp/project".to_string();
        write_session_record(&options, &record).unwrap();

        let found = find_session(
            &options,
            &FindSessionOptions {
                agent_command: record.agent_command.clone(),
                cwd: PathBuf::from("/tmp/project"),
                name: record.name.clone(),
                include_closed: false,
            },
        )
        .unwrap();
        assert_eq!(found.unwrap().acpx_record_id, record.acpx_record_id);
    }
}
