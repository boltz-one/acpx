//! Ports `pruneSessions`.

use std::fs;
use std::path::Path;

use crate::error::{AcpError, Result};
use crate::session::record::SessionRecord;
use crate::session::store_options::{
    AcpFileSessionStoreOptions, ensure_session_dir, safe_session_id,
};

use super::load_record_from_file;
use crate::session::persistence::index::{
    SessionIndexEntry, load_or_rebuild_session_index, rebuild_session_index,
};

pub struct PruneOptions {
    pub agent_command: Option<String>,
    pub before: Option<String>,
    /// Convenience alternative to `before`: prune sessions whose
    /// `closed_at_or_last_used_at` predates "now minus this many
    /// milliseconds". Ports acpx's `PruneOptions.olderThanMs`. Ignored when
    /// `before` is also set (mirrors acpx's `options.before ?? (olderThanMs
    /// != null ? ... : undefined)` precedence — `before` wins).
    pub older_than_ms: Option<u64>,
    pub include_history: bool,
    pub dry_run: bool,
}

/// Resolves the effective prune cutoff: `before` if set, otherwise a cutoff
/// computed from `older_than_ms` (now minus that many milliseconds,
/// formatted the same way `iso_now`/`closed_at_or_last_used_at` are, since
/// cutoff comparisons are plain lexical string comparisons), otherwise
/// `None` (no cutoff — every closed session is eligible). Ports the cutoff
/// half of acpx's `pruneSessions`.
fn resolve_prune_cutoff(query: &PruneOptions) -> Option<String> {
    if let Some(before) = &query.before {
        return Some(before.clone());
    }
    query.older_than_ms.map(|older_than_ms| {
        let cutoff = chrono::Utc::now() - chrono::Duration::milliseconds(older_than_ms as i64);
        cutoff.to_rfc3339()
    })
}

pub struct PruneResult {
    pub pruned: Vec<SessionRecord>,
    pub bytes_freed: u64,
    pub dry_run: bool,
}

fn is_session_stream_file(file_name: &str, safe_id: &str) -> bool {
    file_name == format!("{safe_id}.stream.ndjson")
        || file_name == format!("{safe_id}.stream.lock")
        || file_name.starts_with(&format!("{safe_id}.stream."))
}

fn unlink_counting_bytes(path: &Path) -> u64 {
    let bytes = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let _ = fs::remove_file(path);
    bytes
}

/// Ports `pruneSessions`.
pub fn prune_sessions(
    options: &AcpFileSessionStoreOptions,
    query: &PruneOptions,
) -> Result<PruneResult> {
    ensure_session_dir(options).map_err(|err| AcpError::Other(err.into()))?;
    let entries =
        load_or_rebuild_session_index(options).map_err(|err| AcpError::Other(err.into()))?;

    let eligible: Vec<&SessionIndexEntry> = entries
        .iter()
        .filter(|entry| {
            entry.closed
                && query
                    .agent_command
                    .as_deref()
                    .is_none_or(|cmd| entry.agent_command == cmd)
        })
        .collect();

    let cutoff = resolve_prune_cutoff(query);
    let records: Vec<SessionRecord> = eligible
        .iter()
        .filter_map(|entry| load_record_from_file(options, &entry.file))
        .filter(|record| {
            cutoff
                .as_deref()
                .is_none_or(|cutoff| record.closed_at_or_last_used_at() < cutoff)
        })
        .collect();

    if query.dry_run {
        return Ok(PruneResult {
            pruned: records,
            bytes_freed: 0,
            dry_run: true,
        });
    }

    let session_dir = options.session_dir();
    let dir_entries: Vec<String> = if query.include_history {
        fs::read_dir(&session_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut bytes_freed = 0;
    for record in &records {
        let safe_id = safe_session_id(&record.acpx_record_id);
        bytes_freed += unlink_counting_bytes(&session_dir.join(format!("{safe_id}.json")));
        if query.include_history {
            for name in dir_entries
                .iter()
                .filter(|name| is_session_stream_file(name, &safe_id))
            {
                bytes_freed += unlink_counting_bytes(&session_dir.join(name));
            }
        }
    }

    let _ = rebuild_session_index(options);
    Ok(PruneResult {
        pruned: records,
        bytes_freed,
        dry_run: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::persistence::repository::write::write_session_record;
    use crate::session::persistence::serialize::test_support::sample_session_record;
    use crate::session::store_options::session_file_path;

    fn options(dir: &tempfile::TempDir) -> AcpFileSessionStoreOptions {
        AcpFileSessionStoreOptions::new(dir.path())
    }

    #[test]
    fn prune_dry_run_reports_without_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let options = options(&dir);
        let mut record = sample_session_record();
        record.closed = true;
        write_session_record(&options, &record).unwrap();

        let result = prune_sessions(
            &options,
            &PruneOptions {
                agent_command: None,
                before: None,
                older_than_ms: None,
                include_history: false,
                dry_run: true,
            },
        )
        .unwrap();
        assert!(result.dry_run);
        assert_eq!(result.pruned.len(), 1);
        assert!(session_file_path(&options, &record.acpx_record_id).exists());
    }

    #[test]
    fn prune_real_run_deletes_closed_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let options = options(&dir);
        let mut record = sample_session_record();
        record.closed = true;
        write_session_record(&options, &record).unwrap();

        let result = prune_sessions(
            &options,
            &PruneOptions {
                agent_command: None,
                before: None,
                older_than_ms: None,
                include_history: false,
                dry_run: false,
            },
        )
        .unwrap();
        assert!(!result.dry_run);
        assert_eq!(result.pruned.len(), 1);
        assert!(!session_file_path(&options, &record.acpx_record_id).exists());
    }

    #[test]
    fn prune_skips_open_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let options = options(&dir);
        let record = sample_session_record();
        write_session_record(&options, &record).unwrap();

        let result = prune_sessions(
            &options,
            &PruneOptions {
                agent_command: None,
                before: None,
                older_than_ms: None,
                include_history: false,
                dry_run: false,
            },
        )
        .unwrap();
        assert!(result.pruned.is_empty());
        assert!(session_file_path(&options, &record.acpx_record_id).exists());
    }

    #[test]
    fn older_than_ms_prunes_a_session_older_than_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let options = options(&dir);
        let mut record = sample_session_record();
        record.closed = true; // last_used_at fixture is a fixed 2026-01-01 date, well in the past.
        write_session_record(&options, &record).unwrap();

        let result = prune_sessions(
            &options,
            &PruneOptions {
                agent_command: None,
                before: None,
                older_than_ms: Some(1_000),
                include_history: false,
                dry_run: true,
            },
        )
        .unwrap();
        assert_eq!(result.pruned.len(), 1);
    }

    #[test]
    fn older_than_ms_produces_same_result_as_equivalent_before_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        let options = options(&dir);
        let mut record = sample_session_record();
        record.closed = true;
        write_session_record(&options, &record).unwrap();

        let older_than_ms: u64 = 5_000;
        let hand_computed_before = (chrono::Utc::now()
            - chrono::Duration::milliseconds(older_than_ms as i64))
        .to_rfc3339();

        let via_older_than_ms = prune_sessions(
            &options,
            &PruneOptions {
                agent_command: None,
                before: None,
                older_than_ms: Some(older_than_ms),
                include_history: false,
                dry_run: true,
            },
        )
        .unwrap();
        let via_before = prune_sessions(
            &options,
            &PruneOptions {
                agent_command: None,
                before: Some(hand_computed_before),
                older_than_ms: None,
                include_history: false,
                dry_run: true,
            },
        )
        .unwrap();

        assert_eq!(via_older_than_ms.pruned.len(), via_before.pruned.len());
        assert_eq!(via_older_than_ms.pruned.len(), 1);
    }

    #[test]
    fn before_takes_precedence_over_older_than_ms_when_both_set() {
        let dir = tempfile::tempdir().unwrap();
        let options = options(&dir);
        let mut record = sample_session_record();
        record.closed = true;
        write_session_record(&options, &record).unwrap();

        // `before` is set to a cutoff earlier than the fixture's
        // `last_used_at`, so nothing should be pruned even though
        // `older_than_ms` alone would match everything.
        let result = prune_sessions(
            &options,
            &PruneOptions {
                agent_command: None,
                before: Some("2025-01-01T00:00:00Z".to_string()),
                older_than_ms: Some(1_000),
                include_history: false,
                dry_run: true,
            },
        )
        .unwrap();
        assert!(result.pruned.is_empty());
    }
}
